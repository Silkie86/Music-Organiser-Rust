//! A song from any DAW the app reads, as a new project for another DAW: its arrangement (tempo, time signature,
//! markers, tracks, notes, audio clips) as the readers give it, written by that DAW's writer in `mo-formats`.
//!
//! Safety, the same as every export: the song is only read. The new project goes in the export folder under a name no
//! file has yet ("Name", then "Name (2)"...), opened so that it can't replace a file that's there (`create_new`), and
//! is read back with the app's own reader before it counts. A copy that doesn't read back is taken away again, so a
//! half-written or wrong file is never left behind. Nothing is ever written over.

use std::io::Write;
use std::path::{Path, PathBuf};

use mo_formats::arrangement::{self, Arrangement};
use mo_formats::paths;

use crate::save::safe_file_name;

/// A DAW the app writes new projects for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Daw {
    FlStudio,
}

impl Daw {
    /// Its name, as the library names its saves.
    pub fn name(self) -> &'static str {
        match self {
            Daw::FlStudio => mo_formats::fl::DAW,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Daw::FlStudio => "flp",
        }
    }

    /// Whether `project` is already this DAW's (there's nothing to convert).
    pub fn owns(self, project: &Path) -> bool {
        project.extension().is_some_and(|e| e.eq_ignore_ascii_case(self.extension()))
    }

    fn write(self, song: &Arrangement, title: &str) -> (Vec<u8>, Vec<String>) {
        match self {
            Daw::FlStudio => {
                let w = mo_formats::fl::write::write(song, title, delphi_now());
                (w.bytes, w.left_out)
            }
        }
    }
}

/// A new project written.
#[derive(Clone, Debug, PartialEq)]
pub struct Exported {
    pub file: PathBuf,
    /// What of the song couldn't go across, in plain words.
    pub left_out: Vec<String>,
    /// What the card says.
    pub message: String,
}

/// Where a project's copy for `daw` goes: `<export root>/<song>/<DAW>/<name>.<ending>`.
pub fn file_for(project: &Path, export_root: &Path, daw: Daw, name: &str) -> PathBuf {
    let song = safe_file_name(paths::stem(&project.to_string_lossy()));
    export_root.join(song).join(daw.name()).join(format!("{}.{}", safe_file_name(name), daw.extension()))
}

/// Reads `project` and writes it as a new project for `daw` in the export folder, never over a file that's there.
/// Reads and writes the disk: never on the window's thread.
pub fn copy_for(project: &Path, export_root: &Path, daw: Daw) -> Result<Exported, String> {
    if daw.owns(project) {
        return Err(format!("This is already a {} project.", daw.name()));
    }
    let raw = std::fs::read(project).map_err(|e| format!("Couldn't read the song: {e}"))?;
    let views = arrangement::read_bytes(project, &raw);
    let song = views.into_iter().next().ok_or("The app can't read this kind of project's arrangement.")?;
    if song.lanes.is_empty() && !song.notes.is_empty() {
        return Err(song.notes.join(" "));
    }
    let title = paths::stem(&project.to_string_lossy()).to_owned();
    let (bytes, left_out) = daw.write(&song, &title);
    if bytes.is_empty() {
        return Err("The new project couldn't be put together.".into());
    }
    let stem = paths::stem(&project.to_string_lossy()).to_owned();
    let file = (1..1000)
        .map(|n| if n == 1 { stem.clone() } else { format!("{stem} ({n})") })
        .map(|name| file_for(project, export_root, daw, &name))
        .find(|f| !f.exists())
        .ok_or("Every name for the copy is taken.")?;
    write_new(&file, &bytes).map_err(|e| format!("Couldn't save {}: {e}", file.display()))?;
    // Read back as the app reads that DAW's projects: a copy that doesn't is taken away again.
    let back = arrangement::read_bytes(&file, &bytes);
    let notes = |a: &[Arrangement]| a.first().map_or(0, Arrangement::note_count);
    if back.first().is_none_or(|a| a.notes.iter().any(|n| n.contains("couldn't be read")))
        || notes(&back) < notes(std::slice::from_ref(&song)).min(1)
    {
        let _ = std::fs::remove_file(&file);
        return Err("The new project didn't read back as it was written, so it was taken away again.".into());
    }
    let mut message = format!("Saved a copy for {} as {}.", daw.name(), file.display());
    if !left_out.is_empty() {
        message.push_str(" Not carried over: ");
        message.push_str(&left_out.join(" "));
    }
    Ok(Exported { file, left_out, message })
}

/// Writes `bytes` as a new file: its folders are made, and the file is opened so that it can't be one that's there
/// already. A file that couldn't be written whole is taken away again.
pub fn write_new(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(file)?;
    let written = out.write_all(bytes).and_then(|()| out.sync_all());
    if let Err(e) = written {
        drop(out);
        let _ = std::fs::remove_file(file);
        return Err(e);
    }
    Ok(())
}

/// Now as FL Studio's dates count: days since 30 December 1899, in UTC.
fn delphi_now() -> f64 {
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    // 1 January 1970 is day 25569.
    25_569.0 + seconds / 86_400.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mo-convert-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_new_file_is_never_written_over_one_thats_there() {
        let d = dir("write-new");
        let file = d.join("a").join("Song.flp");
        write_new(&file, b"first").unwrap();
        assert!(write_new(&file, b"second").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"first");
    }

    #[test]
    fn a_studio_one_song_becomes_an_fl_project_beside_the_last_copy() {
        let d = dir("copy-for");
        let song = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mo-formats/tests/fixtures/recipe/studio-one/MO Recipe 02 notes.song");
        let root = d.join("Exports");
        let first = copy_for(&song, &root, Daw::FlStudio).unwrap();
        assert_eq!(first.file, root.join("MO Recipe 02 notes").join("FL Studio").join("MO Recipe 02 notes.flp"));
        let second = copy_for(&song, &root, Daw::FlStudio).unwrap();
        assert_eq!(second.file.file_name().unwrap(), "MO Recipe 02 notes (2).flp");
        let back = arrangement::read(&first.file).unwrap().remove(0);
        let original = arrangement::read(&song).unwrap().remove(0);
        assert_eq!(back.note_count(), original.note_count());
        assert!(back.note_count() > 0);
        assert!(copy_for(&first.file, &root, Daw::FlStudio).is_err(), "an FL project isn't converted for FL");
    }
}
