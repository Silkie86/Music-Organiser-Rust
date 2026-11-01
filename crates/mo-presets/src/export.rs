//! Exporting a song's sounds as preset files a plugin (or another DAW) can load, as the C# `Exporter.Presets` does:
//! into `<export folder>\<song's save name>\Presets`, each named "Plugin - Name (Where).ext". Exporting again writes
//! the same names over the last export, so exporting one plugin after another builds one folder without copies.
//!
//! Each sound's name is worked out among all its save's sounds, so it's the same whichever of them are exported: the
//! second of two "Serum" channels is "Serum 2.fxp" even when it's exported on its own, and never takes the first one's
//! file (the C# app named only what was exported, so it did).
//!
//! Each file is written beside its place under a name of its own, made sure of on the disk, read back and checked, then
//! renamed into place (copy, check, rename), so a half-written preset never sits under the real name.
//!
//! Given Bitwig templates ([`crate::bitwig`]), a sound whose plugin has one also gets a Bitwig preset beside its
//! plugin preset ("Serum - Wobble.bwpreset"), as the C# export does. Each sound's `.bwpreset` name is kept for it
//! whether or not one is made, so names never shift when a template is added. A preset that
//! can't be written is listed with why, and the others still go. Nothing here touches a project or a preset you made:
//! only the export folder is written to.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use mo_formats::paths;
use mo_formats::presets::PresetBlob;

use crate::bitwig::{self, Templates};
use crate::files;
use crate::filter::Choice;
use crate::names;
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
/// Each name's Bitwig preset ([`bitwig_name`]) is kept for it too, so "Pad.fxp" and "Pad.vstpreset" never share
/// "Pad.bwpreset".
pub fn file_names(presets: &[PresetBlob]) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    presets
        .iter()
        .map(|p| {
            let label = file_label(p);
            let ext = safe_extension(&p.extension);
            let mut name = format!("{label}{ext}");
            let mut n = 2;
            loop {
                let keys = [name.to_uppercase(), bitwig_name(&name, &ext).to_uppercase()];
                if keys.iter().all(|k| !used.contains(k)) {
                    used.extend(keys);
                    break;
                }
                name = format!("{label} {n}{ext}");
                n += 1;
            }
            name
        })
        .collect()
}

/// The Bitwig preset's name beside an exported preset, from its name and (safe) ending: "Serum - Wobble.fxp" gives
/// "Serum - Wobble.bwpreset". Only the ending is taken off, never what's after a dot in the sound's own name.
fn bitwig_name(file_name: &str, extension: &str) -> String {
    format!("{}.bwpreset", file_name.strip_suffix(extension).unwrap_or(file_name))
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

/// Writes presets for the save at `save` (its path, for the folder's name) into the export folder, each with a Bitwig
/// preset beside it when `templates` has one for its plugin ([`bitwig::templates`]; `None` when Send to Bitwig is
/// off). A preset that can't be written is listed in
/// [`Exported::failed`] and the rest still go; it's an error when none could be.
pub fn export(
    presets: &[PresetBlob],
    save: &str,
    export_root: &Path,
    templates: Option<&Templates>,
) -> io::Result<Exported> {
    write_all(presets.iter().zip(file_names(presets)).collect(), Vec::new(), save, export_root, templates)
}

/// Writes each preset under its name, after `failed` (what was missed before writing). An export that wrote nothing
/// and missed something is an error.
fn write_all(
    chosen: Vec<(&PresetBlob, String)>,
    mut failed: Vec<String>,
    save: &str,
    export_root: &Path,
    templates: Option<&Templates>,
) -> io::Result<Exported> {
    if chosen.is_empty() && !failed.is_empty() {
        return Err(io::Error::other(failed.join("; ")));
    }
    let folder = folder_for(export_root, save);
    std::fs::create_dir_all(&folder)?;
    clear_half_written(&folder, LEFT_BEHIND);
    let mut files = Vec::new();
    // Each template's file, read once for the whole export.
    let mut template_bytes: HashMap<PathBuf, Result<Vec<u8>, String>> = HashMap::new();
    for (p, name) in chosen {
        // Settings let go once listed (`SongPreset::forget_data`) are never written as an empty file over the last
        // export: a reader never gives a sound with none.
        let written = if p.data.is_empty() {
            Err(io::Error::other("its settings were let go after it was listed, so it's exported from its save"))
        } else {
            write_checked(&folder.join(&name), &p.data)
        };
        match written {
            Ok(()) => {
                let bw = bitwig_name(&name, &safe_extension(&p.extension));
                files.push(name);
                // Send to Bitwig: only beside a plugin preset that was written.
                let Some(templates) = templates else { continue };
                let made = match bitwig::template_for(p, templates) {
                    Some(template) => {
                        let raw = template_bytes
                            .entry(template.path.clone())
                            .or_insert_with(|| {
                                std::fs::read(&template.path).map_err(|e| format!("its template couldn't be read: {e}"))
                            })
                            .as_deref()
                            .map_err(|e| e.clone());
                        // A template changed since it was found could trip the reader: that's this one's failure.
                        let built = raw.and_then(|raw| {
                            std::panic::catch_unwind(|| bitwig::build(raw, &template.path, p, &p.name))
                                .unwrap_or_else(|_| Err("its template stopped the reader".into()))
                        });
                        Some(built.map_err(io::Error::other).and_then(|b| write_checked(&folder.join(&bw), &b)))
                    }
                    None => None,
                };
                match made {
                    Some(Ok(())) => files.push(bw),
                    Some(Err(e)) => failed.push(stale(&folder, &bw, &e.to_string())),
                    // No template for its plugin now, but one was made last time: it no longer matches.
                    None if folder.join(&bw).exists() => {
                        failed.push(stale(&folder, &bw, "no preset Bitwig saved for its plugin was found"))
                    }
                    None => {}
                }
            }
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    if files.is_empty() && !failed.is_empty() {
        return Err(io::Error::other(failed.join("; ")));
    }
    Ok(Exported { folder, files, failed, warning: None })
}

/// Why a Bitwig preset wasn't made, saying when one from an earlier export is still there with older settings.
fn stale(folder: &Path, bw: &str, why: &str) -> String {
    if folder.join(bw).exists() {
        format!("{bw}: {why} (the one from an earlier export is still there, with the settings it had then)")
    } else {
        format!("{bw}: {why}")
    }
}

/// How old a half-written file must be before it's taken as left behind by a crash: no export takes this long to
/// write one file, so one still being written by another export is never cleared.
const LEFT_BEHIND: Duration = Duration::from_secs(10 * 60);

/// Clears half-written files ([`write_checked`]'s "<name>.part-<app>-<write>") a crash left in an export folder, once
/// they're older than `older_than`. Only names of exactly that shape are touched, in this folder only (not inside
/// others), so nothing else is ever deleted; one that can't be cleared is left for next time.
pub fn clear_half_written(folder: &Path, older_than: Duration) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !kind.is_file() || !is_half_written(name) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age >= older_than);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A half-written file's name: something, then ".part-", digits, "-", digits, and nothing after.
fn is_half_written(name: &str) -> bool {
    let Some((before, tail)) = name.rsplit_once(".part-") else { return false };
    let Some((app, write)) = tail.split_once('-') else { return false };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    !before.is_empty() && digits(app) && digits(write)
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
    templates: Option<&Templates>,
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
    let mut exported = write_all(chosen, failed, save, export_root, templates)?;
    exported.warning = warning.map(str::to_owned);
    Ok(exported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_a_half_written_name() {
        assert!(is_half_written("Serum - Lead.fxp.part-1234-0"));
        assert!(is_half_written("a.part-1-22"));
        for name in [
            "Serum - Lead.fxp",
            "x.part-12",
            "x.part-12-",
            "x.part-1a-2",
            ".part-1-2",
            "x.part-1-2.fxp",
            "Body part-1-2",
        ] {
            assert!(!is_half_written(name), "{name}");
        }
    }

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
