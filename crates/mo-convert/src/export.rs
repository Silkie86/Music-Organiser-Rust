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
    AbletonLive,
}

impl Daw {
    /// Its name, as the library names its saves.
    pub fn name(self) -> &'static str {
        match self {
            Daw::FlStudio => mo_formats::fl::DAW,
            Daw::StudioOne => mo_formats::studio_one::DAW,
            Daw::AbletonLive => mo_formats::ableton::DAW,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Daw::FlStudio => "flp",
            Daw::StudioOne => "song",
            Daw::AbletonLive => "als",
        }
    }

    /// Whether `project` is already this DAW's (there's nothing to convert).
    pub fn owns(self, project: &Path) -> bool {
        project.extension().is_some_and(|e| e.eq_ignore_ascii_case(self.extension()))
    }

    /// The song as a project for this DAW, to go in `folder`; and what couldn't go across.
    fn write(self, song: &Arrangement, title: &str, folder: &Path) -> (Vec<u8>, Vec<String>) {
        // What Studio One and Live keep about each audio file, from its header, when it's on this computer.
        let format = |path: &str| {
            crate::sample::format_of(Path::new(path)).map(|(rate, channels, seconds)| {
                mo_formats::studio_one::write::SampleFormat { rate, channels, seconds }
            })
        };
        match self {
            Daw::FlStudio => {
                let w = mo_formats::fl::write::write(song, title, delphi_now());
                (w.bytes, w.left_out)
            }
            Daw::StudioOne => {
                let w = mo_formats::studio_one::write::write(song, title, &format);
                (w.bytes, w.left_out)
            }
            // A set has no title of its own: its file's name is its name.
            Daw::AbletonLive => {
                let w = mo_formats::ableton::write::write(song, &format, Some(&folder.to_string_lossy()));
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
    let views = arrangement::read_bytes(project, &raw);
    let mut song = views.into_iter().next().ok_or("The app can't read this kind of project's arrangement.")?;
    if song.lanes.is_empty() && !song.notes.is_empty() {
        return Err(song.notes.join(" "));
    }
    let missing = find_samples(&mut song, find);
    let stem = paths::stem(&project.to_string_lossy()).to_owned();
    let folder = file_for(project, export_root, daw, &stem).parent().map(Path::to_path_buf).unwrap_or_default();
    let (bytes, mut left_out) = daw.write(&song, &stem, &folder);
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
        // And as a Live set, which reads back the same too.
        let set = copy_for(&song, &root, Daw::AbletonLive, &as_named).unwrap();
        assert_eq!(set.file, root.join("MO Recipe 02 notes").join("Ableton Live").join("MO Recipe 02 notes.als"));
        assert_eq!(arrangement::read(&set.file).unwrap().remove(0).note_count(), original.note_count());
        assert!(
            copy_for(&set.file, &root, Daw::AbletonLive, &as_named).is_err(),
            "a Live set isn't converted for Live"
        );
    }

    #[test]
    fn a_bitwig_song_s_audio_goes_into_a_live_set_by_its_path_and_from_the_set_s_folder() {
        let d = dir("copy-for-live");
        let song = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mo-formats/tests/fixtures/recipe/bitwig/MO Recipe 12 gain and fades.bwproject");
        let root = d.join("Exports");
        let sample = d.join("Samples").join("Tone.wav");
        let found = sample.to_string_lossy().into_owned();
        let set = copy_for(&song, &root, Daw::AbletonLive, &|_| Some(found.clone())).unwrap();
        let back = arrangement::read(&set.file).unwrap().remove(0);
        let parts: Vec<_> = back.lanes.iter().flat_map(|l| &l.clips).flat_map(|c| &c.audio).collect();
        assert!(!parts.is_empty());
        assert!(parts.iter().all(|p| Path::new(&p.path) == sample), "{parts:?}");
        let mut xml = String::new();
        std::io::Read::read_to_string(
            &mut flate2::read::GzDecoder::new(&std::fs::read(&set.file).unwrap()[..]),
            &mut xml,
        )
        .unwrap();
        assert!(xml.contains("<RelativePath Value=\"../../../Samples/Tone.wav\" />"), "{xml}");
    }
}
