//! The whole page's list: the sounds in many songs' saves, and the preset files in preset folders, read over every
//! core of the current rayon pool (rule 21). Call these from the app's low-priority pool (`mo_app::background::on_pool`),
//! never on the window's thread: they ask the disk.
//!
//! Each save is read once, and its sounds' settings are let go as soon as it's listed (exporting reads it again). A
//! file that can't be read, or that trips a reader, is listed with what went wrong; it never stops the others.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mo_formats::bitwig::Project;
use mo_formats::fl::events;
use mo_formats::fl::presets::extract_presets;
use mo_formats::fl::song::read_song;
use mo_formats::{PluginKind, paths};
use rayon::prelude::*;

use crate::files::{self, FileFormat, NotAPreset, PresetFile};
use crate::list::{self, SongPreset};
use crate::role::{Basis, Guess, Thresholds, name_clue};

/// The sounds in one save.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveSounds {
    pub path: String,
    /// Its sounds, their settings let go.
    pub sounds: Vec<SongPreset>,
    /// Why it couldn't be read, when it couldn't.
    pub error: Option<String>,
    /// Something off about a save that was still read (an FL save cut short, or with a garbled header: rule 9).
    pub warning: Option<String>,
}

/// The DAW a save is for, by its extension: only FL Studio and Bitwig saves give their plugins' settings.
fn daw_of(path: &str) -> Option<&'static str> {
    let ext = paths::extension(path).trim_start_matches('.');
    if ext.eq_ignore_ascii_case("flp") {
        Some("FL Studio")
    } else if ext.eq_ignore_ascii_case("bwproject") {
        Some("Bitwig Studio")
    } else {
        None
    }
}

/// Reads one save's sounds. `None` for a save whose DAW's projects don't keep readable settings (Studio One).
pub fn read_save(path: &str, limits: &Thresholds) -> Option<SaveSounds> {
    let daw = daw_of(path)?;
    let failed =
        |error: String| SaveSounds { path: path.to_owned(), sounds: Vec::new(), error: Some(error), warning: None };
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) => return Some(failed(e.to_string())),
    };
    // A reader that trips over a damaged file is caught here, as the library's scanner does.
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match daw {
        // As `list::read_fl`, keeping the reader's own words for a damaged save and its warning for one read on.
        "FL Studio" => match events::load(&raw) {
            Ok(flp) => Ok((list::from_fl(&read_song(&flp), extract_presets(&flp), limits), flp.warning)),
            Err(_) => Err(events::DAMAGED.to_owned()),
        },
        _ => match Project::open(raw) {
            Ok(project) => Ok((list::from_bitwig(&project, limits), None)),
            Err(_) => Err("it isn't a Bitwig project that can be decoded".to_owned()),
        },
    }));
    Some(match read {
        Ok(Ok((mut sounds, warning))) => {
            sounds.iter_mut().for_each(SongPreset::forget_data);
            SaveSounds { path: path.to_owned(), sounds, error: None, warning: warning.map(str::to_owned) }
        }
        Ok(Err(e)) => failed(e),
        Err(_) => failed("the reader stopped on something it didn't expect in this file".into()),
    })
}

/// Reads many saves at once, in the order given; saves of other DAWs are left out.
pub fn read_saves(paths: &[String], limits: &Thresholds) -> Vec<SaveSounds> {
    paths.par_iter().filter_map(|p| read_save(p, limits)).collect()
}

/// A preset file found in a folder.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundPreset {
    pub path: String,
    pub file: Result<PresetFile, NotAPreset>,
}

/// How deep a preset folder is walked: preset libraries nest by maker, plugin, pack and category.
const DEEPEST: usize = 16;

/// Every preset file under `roots`, a level of folders at a time over every core, each file's header read. Folders
/// that are linked in are walked once, however they link back; folders starting with "." are skipped (Bitwig's
/// ".cache"); a root that isn't there is skipped. A file that trips its reader is listed with what went wrong; it
/// never stops the others.
pub fn find_preset_files(roots: &[PathBuf]) -> Vec<FoundPreset> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut level: Vec<PathBuf> = Vec::new();
    for r in roots {
        if r.is_dir() && seen.insert(real_path(r)) {
            level.push(r.clone());
        }
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for _ in 0..DEEPEST {
        if level.is_empty() {
            break;
        }
        let listed: Vec<_> = level.par_iter().map(|d| list_dir(d)).collect();
        level = Vec::new();
        for (dirs, found) in listed {
            files.extend(found);
            for (d, real) in dirs {
                if seen.insert(real) {
                    level.push(d);
                }
            }
        }
    }
    files.sort();
    files
        .par_iter()
        .map(|f| {
            // A reader that trips over a damaged file is caught here, as `read_save` does.
            let read = std::panic::catch_unwind(|| files::read(f));
            FoundPreset {
                path: f.to_string_lossy().into_owned(),
                file: match read {
                    Ok(Ok(read)) => read,
                    Ok(Err(e)) => Err(NotAPreset(e.to_string())),
                    Err(_) => Err(NotAPreset("the reader stopped on something it didn't expect in this file".into())),
                },
            }
        })
        .collect()
}

/// Where a folder really is, so a link back to one already walked isn't walked again.
fn real_path(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// A folder's folders (each with where it really is, worked out here on the pool rather than one at a time after)
/// and preset files. Names that aren't Unicode are left out, as the library's scanner does, so a listed path always
/// opens again.
fn list_dir(dir: &Path) -> (Vec<(PathBuf, PathBuf)>, Vec<PathBuf>) {
    let (mut dirs, mut found) = (Vec::new(), Vec::new());
    let Ok(entries) = std::fs::read_dir(dir) else { return (dirs, found) };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if path.to_str().is_none() {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() || (t.is_symlink() && path.is_dir()) => {
                if !name.starts_with('.') {
                    let real = real_path(&path);
                    dirs.push((path, real));
                }
            }
            Ok(_) if FileFormat::of(name).is_some() => found.push(path),
            _ => {}
        }
    }
    (dirs, found)
}

/// Where plugins and DAWs keep presets you saved, on this computer, that are there: the VST3 preset folders, and
/// Bitwig's (Documents\Bitwig Studio, as the C# app looks).
pub fn default_folders() -> Vec<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    let mut out = Vec::new();
    if let Some(home) = &home {
        if cfg!(windows) {
            out.push(home.join("Documents").join("VST3 Presets"));
            out.push(home.join("Documents").join("Bitwig Studio"));
        } else if cfg!(target_os = "macos") {
            out.push(home.join("Library").join("Audio").join("Presets"));
            out.push(home.join("Documents").join("Bitwig Studio"));
        } else {
            out.push(home.join(".vst3").join("presets"));
            out.push(home.join("Bitwig Studio"));
        }
    }
    if cfg!(windows)
        && let Some(common) = std::env::var_os("CommonProgramFiles")
    {
        out.push(PathBuf::from(common).join("VST3 Presets"));
    }
    out.retain(|p| p.is_dir());
    out
}

/// Where a row on the page comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// A sound inside a song's save. `key` says which sound it is within its save ([`SongPreset::place_key`]), so it
    /// can be found again to export it or keep a fix to its part: two FL channels of one plugin share a name and place.
    Song { save: String, key: String },
    /// A preset file on disk.
    File { path: String },
}

/// One line on the page, a song's sound or a preset file alike.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetRow {
    pub source: Source,
    pub name: String,
    pub plugin: String,
    pub kind: PluginKind,
    /// One of the DAW's own plugins.
    pub native: bool,
    /// The channel, track or insert it's on in its song; a file's folder.
    pub place: String,
    /// The file type it exports or loads as (".fxp", ".bwpreset"...).
    pub extension: String,
    /// The part it plays: a song's sound is guessed; a preset file has none (it never played in a song), unless its
    /// category or name gives a clue.
    pub guess: Guess,
    pub plays_nothing: bool,
}

impl PresetRow {
    /// The texts a search looks in: name, plugin, where it is, and the song's or file's name.
    pub fn search_texts(&self) -> [&str; 4] {
        let (Source::Song { save: path, .. } | Source::File { path }) = &self.source;
        [&self.name, &self.plugin, &self.place, paths::stem(path)]
    }
}

/// The page's rows from saves' sounds and found preset files (files that aren't presets left out).
pub fn rows(saves: &[SaveSounds], found: &[FoundPreset]) -> Vec<PresetRow> {
    let mut out = Vec::new();
    for s in saves {
        out.extend(s.sounds.iter().map(|p| PresetRow {
            source: Source::Song { save: s.path.clone(), key: p.place_key() },
            name: p.blob.name.clone(),
            plugin: p.blob.plugin.clone(),
            kind: p.blob.kind,
            native: p.blob.native,
            place: p.track.clone(),
            extension: p.blob.extension.to_string(),
            guess: p.guess.clone(),
            plays_nothing: p.plays_nothing,
        }));
    }
    for f in found {
        let Ok(file) = &f.file else { continue };
        let ext = paths::extension(&f.path).to_lowercase();
        let folder = Path::new(&f.path).parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        let clue = [file.category.as_str(), file.name.as_str()]
            .into_iter()
            .find_map(|t| name_clue(t).map(|(role, word)| (role, word, t.to_owned())));
        let guess = match clue {
            Some((role, word, text)) => Guess {
                role: Some(role),
                basis: Basis::Name,
                reason: format!("A preset file: \"{text}\" says {} (\"{word}\")", role.label().to_lowercase()),
                octave_checked: true,
                weak: true,
            },
            None => Guess::none("A preset file: it never played in a song"),
        };
        out.push(PresetRow {
            source: Source::File { path: f.path.clone() },
            name: file.name.clone(),
            plugin: if file.plugin.is_empty() { file.plugin_id.clone() } else { file.plugin.clone() },
            kind: file.kind,
            // A Bitwig preset of one of Bitwig's own devices is the DAW's own, as an FL plugin's state is.
            native: file.format == Some(FileFormat::BwPreset) && !file.wraps_plugin,
            place: folder.unwrap_or_default(),
            extension: ext,
            guess,
            plays_nothing: false,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fl_and_bitwig_saves_are_read() {
        assert_eq!(daw_of(r"C:\x\Song.FLP"), Some("FL Studio"));
        assert_eq!(daw_of("/x/Song.bwproject"), Some("Bitwig Studio"));
        assert_eq!(daw_of("/x/Song.song"), None);
        assert!(read_save("/x/Song.song", &Thresholds::default()).is_none());
    }
}
