//! A song from any DAW the app reads, as a new project for another DAW: its arrangement (tempo, time signature,
//! markers, tracks, notes, audio clips) as the readers give it, written by that DAW's writer in `mo-formats`.
//!
//! Safety, the same as every export: the song is only read. The new project goes in the export folder under a name no
//! file has yet ("Name", then "Name (2)"...). It's written under a temporary name first, read back from the disk with
//! the app's own reader and checked (every note and every audio clip there), and only then given its name, in a way
//! that fails when a file has that name ([`crate::save`]). A copy that doesn't check out is taken away again, so a
//! half-written or wrong file is never left behind. Nothing is ever written over.

use std::path::{Path, PathBuf};

use mo_formats::arrangement::{self, Arrangement};
use mo_formats::paths;

use crate::save::{LONGEST_FILE_NAME, cut, safe_file_name, save_checked_new};

/// A DAW the app writes new projects for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Daw {
    FlStudio,
    StudioOne,
}

impl Daw {
    /// Its name, as the library names its saves.
    pub fn name(self) -> &'static str {
        match self {
            Daw::FlStudio => mo_formats::fl::DAW,
            Daw::StudioOne => mo_formats::studio_one::DAW,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Daw::FlStudio => "flp",
            Daw::StudioOne => "song",
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
            Daw::StudioOne => {
                // What Studio One keeps about each audio file, from its header, when it's on this computer.
                let format = |path: &str| {
                    crate::sample::format_of(Path::new(path)).map(|(rate, channels, seconds)| {
                        mo_formats::studio_one::write::SampleFormat { rate, channels, seconds }
                    })
                };
                let w = mo_formats::studio_one::write::write(song, title, &format);
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

/// The names a copy may have: the song's own, then "Name (2)", "Name (3)"... A long name is shortened to leave room
/// for the number, so each number is its own file.
fn names(stem: &str) -> impl Iterator<Item = String> + '_ {
    (1..1000).map(move |n| {
        if n == 1 {
            safe_file_name(stem)
        } else {
            let number = format!(" ({n})");
            format!("{}{number}", cut(stem, LONGEST_FILE_NAME - number.len()))
        }
    })
}

/// Reads `project` and writes it as a new project for `daw` in the export folder, never over a file that's there.
/// `find` says where a sample the song names is on this computer (it may have moved, or be named from the song's
/// folder): the new project names the file there. Reads and writes the disk: never on the window's thread.
pub fn copy_for(
    project: &Path,
    export_root: &Path,
    daw: Daw,
    find: &dyn Fn(&str) -> Option<String>,
) -> Result<Exported, String> {
    if daw.owns(project) {
        return Err(format!("This is already a {} project.", daw.name()));
    }
    let raw = std::fs::read(project).map_err(|e| format!("Couldn't read the song: {e}"))?;
    let mut views = arrangement::read_bytes(project, &raw);
    // The first view, unless nothing plays in it (no notes, no audio) and something does in another: an FL song
    // played in pattern mode has no patterns in its playlist (an automation clip may be there on its own), and its
    // second view lays its patterns out one after another, as it opens in Bitwig (1,743 of Solomon's FL saves in the
    // real-song test of 27 Nov 2026 couldn't be copied before).
    let plays = |v: &Arrangement| counts(v) != (0, 0);
    let chosen = if views.first().is_some_and(plays) { 0 } else { views.iter().position(plays).unwrap_or(0) };
    let first_view = views.first().map(|v| v.view);
    if views.is_empty() {
        return Err("The app can't read this kind of project's arrangement.".into());
    }
    let mut song = views.swap_remove(chosen);
    let why_not_first = first_view
        .filter(|_| chosen > 0)
        .map(|first| format!("The song's {first} is empty, so the copy is laid out \"{}\".", song.view));
    if song.lanes.is_empty() && !song.notes.is_empty() {
        return Err(song.notes.join(" "));
    }
    let missing = find_samples(&mut song, find);
    let stem = paths::stem(&project.to_string_lossy()).to_owned();
    let (bytes, mut left_out) = daw.write(&song, &stem);
    if bytes.is_empty() {
        return Err("The new project couldn't be put together.".into());
    }
    if !missing.is_empty() {
        left_out.push(format!(
            "Samples not found on this computer (the copy names them as the song does): {}.",
            missing.join(", ")
        ));
    }
    let wanted = counts(&song);
    // Read back as the app reads that DAW's projects, from the disk: every note and audio clip must be there.
    let check = |file: &Path, written: &[u8]| -> Option<String> {
        let back = arrangement::read_bytes(file, written);
        match back.first() {
            None => Some("it didn't read back".into()),
            Some(a) if a.notes.iter().any(|n| n.contains("couldn't be read")) => Some("it didn't read back".into()),
            Some(a) if counts(a) != wanted => Some("it didn't read back as it was written".into()),
            Some(_) => None,
        }
    };
    for name in names(&stem) {
        let file = file_for(project, export_root, daw, &name);
        if file.exists() {
            continue;
        }
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder).map_err(|e| format!("Couldn't make {}: {e}", folder.display()))?;
        }
        // False when the name was taken while this saved (a second press, another program): the next name is tried.
        if save_checked_new(&file, &bytes, |b| check(&file, b))? {
            let mut message = format!("Saved a copy for {} as {}.", daw.name(), file.display());
            // Why the copy is laid out as it is, before what didn't go across.
            if let Some(why) = &why_not_first {
                message.push(' ');
                message.push_str(why);
            }
            if !left_out.is_empty() {
                message.push_str(" Not carried over: ");
                message.push_str(&left_out.join(" "));
            }
            return Ok(Exported { file, left_out, message });
        }
    }
    Err("Every name for the copy is taken.".into())
}

/// The song's notes and audio clips, as a written copy must keep them.
fn counts(song: &Arrangement) -> (usize, usize) {
    let audio = song.lanes.iter().flat_map(|l| &l.clips).map(|c| c.audio.len()).sum();
    (song.note_count(), audio)
}

/// Points each audio clip at its sample where it is on this computer; the samples that aren't anywhere, by name.
fn find_samples(song: &mut Arrangement, find: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for part in song.lanes.iter_mut().flat_map(|l| &mut l.clips).flat_map(|c| &mut c.audio) {
        match find(&part.path) {
            Some(found) => part.path = found,
            None => {
                let name = paths::file_name(&part.path).to_owned();
                if !missing.contains(&name) {
                    missing.push(name);
                }
            }
        }
    }
    missing
}

/// Now as FL Studio's dates count: days since 30 December 1899, in the computer's own time (as FL dates a project).
fn delphi_now() -> f64 {
    let now = chrono::Local::now();
    let seconds = now.timestamp() as f64 + f64::from(now.timestamp_subsec_millis()) / 1000.0;
    let local = seconds + f64::from(now.offset().local_minus_utc());
    // 1 January 1970 is day 25569.
    25_569.0 + local / 86_400.0
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

    /// Every sample where the song names it.
    fn as_named(sample: &str) -> Option<String> {
        Some(sample.to_owned())
    }

    #[test]
    fn a_file_thats_there_is_never_written_over() {
        let d = dir("not-over");
        let song = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mo-formats/tests/fixtures/recipe/studio-one/MO Recipe 02 notes.song");
        let root = d.join("Exports");
        let there = file_for(&song, &root, Daw::FlStudio, "MO Recipe 02 notes");
        std::fs::create_dir_all(there.parent().unwrap()).unwrap();
        std::fs::write(&there, b"mine").unwrap();
        let copy = copy_for(&song, &root, Daw::FlStudio, &as_named).unwrap();
        assert_eq!(copy.file.file_name().unwrap(), "MO Recipe 02 notes (2).flp");
        assert_eq!(std::fs::read(&there).unwrap(), b"mine");
        // Nothing of the save's temporary name is left.
        let left: Vec<_> =
            std::fs::read_dir(there.parent().unwrap()).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
    }

    #[test]
    fn a_long_name_is_shortened_to_leave_room_for_its_number() {
        let long = "x".repeat(200);
        let all: Vec<String> = names(&long).take(3).collect();
        assert_eq!(all[0].chars().count(), LONGEST_FILE_NAME);
        assert!(all[1].ends_with(" (2)") && all[1].chars().count() == LONGEST_FILE_NAME);
        assert_ne!(all[1], all[2]);
    }

    #[test]
    fn a_sample_found_elsewhere_is_named_where_it_is_and_one_found_nowhere_is_said() {
        let d = dir("samples");
        let song = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mo-formats/tests/fixtures/recipe/studio-one/MO Recipe 04 audio clip.song");
        let nowhere = copy_for(&song, &d, Daw::FlStudio, &|_| None).unwrap();
        assert!(nowhere.left_out.iter().any(|l| l.starts_with("Samples not found")), "{:?}", nowhere.left_out);
        let found = copy_for(&song, &d, Daw::FlStudio, &|_| Some(r"D:\Found\Loop.wav".to_owned())).unwrap();
        assert!(!found.left_out.iter().any(|l| l.starts_with("Samples not found")));
        let info = mo_formats::fl::read_bytes(&std::fs::read(&found.file).unwrap());
        assert!(
            !info.samples.is_empty() && info.samples.iter().all(|s| s == r"D:\Found\Loop.wav"),
            "{:?}",
            info.samples
        );
    }

    /// Recipe save 02 with nothing in its playlist: FL plays its pattern in pattern mode.
    fn pattern_mode_flp(dir: &Path) -> PathBuf {
        let raw = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../mo-formats/tests/fixtures/recipe/fl/MO Recipe 02 notes.flp"),
        )
        .unwrap();
        let flp = mo_formats::fl::events::load(&raw).unwrap();
        let mut body = Vec::new();
        for e in flp.events.iter().filter(|e| e.id != mo_formats::fl::events::EV_PLAYLIST) {
            body.push(e.id);
            if e.id >= 192 {
                let mut n = e.data.len();
                while n >= 0x80 {
                    body.push((n & 0x7f) as u8 | 0x80);
                    n >>= 7;
                }
                body.push(n as u8);
            }
            body.extend_from_slice(&e.data);
        }
        // The header as it was (FLhd, its 6 bytes, FLdt), then the events' new length.
        let mut out = raw[..18].to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
        let file = dir.join("Pattern Mode.flp");
        std::fs::write(&file, out).unwrap();
        file
    }

    #[test]
    fn a_song_in_fls_pattern_mode_is_copied_as_it_opens_in_bitwig() {
        // 1,743 of Solomon's FL saves are played in pattern mode (real-song test, 27 Nov 2026): their playlist is
        // empty, so a copy from it had nothing in it and wasn't made.
        let d = dir("pattern-mode");
        let flp = pattern_mode_flp(&d);
        let views = arrangement::read(&flp).unwrap();
        assert!(views[0].lanes.is_empty() && views[1].note_count() > 0, "{views:?}");
        let copy = copy_for(&flp, &d.join("Exports"), Daw::StudioOne, &as_named).unwrap();
        assert!(copy.message.contains("so the copy is laid out \"As it opens in Bitwig\""), "{}", copy.message);
        assert!(!copy.left_out.iter().any(|l| l.contains("laid out")), "{:?}", copy.left_out);
        let back = arrangement::read(&copy.file).unwrap().remove(0);
        assert_eq!(back.note_count(), views[1].note_count());
    }

    #[test]
    fn a_studio_one_song_becomes_an_fl_project_beside_the_last_copy() {
        let d = dir("copy-for");
        let song = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mo-formats/tests/fixtures/recipe/studio-one/MO Recipe 02 notes.song");
        let root = d.join("Exports");
        let first = copy_for(&song, &root, Daw::FlStudio, &as_named).unwrap();
        assert_eq!(first.file, root.join("MO Recipe 02 notes").join("FL Studio").join("MO Recipe 02 notes.flp"));
        let second = copy_for(&song, &root, Daw::FlStudio, &as_named).unwrap();
        assert_eq!(second.file.file_name().unwrap(), "MO Recipe 02 notes (2).flp");
        let back = arrangement::read(&first.file).unwrap().remove(0);
        let original = arrangement::read(&song).unwrap().remove(0);
        assert_eq!(back.note_count(), original.note_count());
        assert!(back.note_count() > 0);
        assert!(
            copy_for(&first.file, &root, Daw::FlStudio, &as_named).is_err(),
            "an FL project isn't converted for FL"
        );
        // And the FL copy, as a Studio One song: the same notes again.
        let song_copy = copy_for(&first.file, &root, Daw::StudioOne, &as_named).unwrap();
        assert_eq!(song_copy.file.extension().unwrap(), "song");
        assert_eq!(arrangement::read(&song_copy.file).unwrap().remove(0).note_count(), original.note_count());
    }
}
