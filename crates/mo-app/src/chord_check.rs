//! Chord check's takes on disk: what Solomon played into the test plugin and what he said each chord really is
//! (the plan "Chord test bench in the plugin", 13 Nov). Each take is one small file in the data folder's
//! [`FOLDER`], in the theory bench's own format (one labelled piece: `tools/theorybench/src/item.rs`), so the bench
//! can read the folder as it reads a dataset (its next step). The bench scores only the stretches labelled
//! ([`TakeFile::chords`]): what isn't labelled, or was skipped, isn't counted right or wrong. They're his (like his recordings), so the start-up tidy (rule 28) never
//! removes them; it only clears a take whose writing was cut short ([`is_half_written`]). Rule 4: never committed.
//! Rule 5 (Solomon, 13 Nov): only measured on, never learned from.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The data folder's folder for the takes.
pub const FOLDER: &str = "Chord check";
/// The bench's name for Solomon's own music (`theorybench`'s `item::OWN`, word for word): only counts and averages
/// from it go in code (rule 5), and nothing is learned from his takes.
pub const OWN: &str = "Solomon's own (rule 5)";
/// Where a take came from, as the bench lists it.
pub const SOURCE: &str = "chord-check";
/// What a take file's name starts and ends with: `take 2026-11-13 07-12-03-250.json`.
const START: &str = "take ";
const END: &str = ".json";
/// What a take is written as first, before it's checked and renamed (copy, check, rename).
const PART: &str = ".mo-part";

/// One take as saved: the bench's fields first (a piece's notes by part, its grid and its chord labels), then what
/// only Chord check keeps ([`Check`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakeFile {
    pub source: String,
    pub id: String,
    /// Ticks a beat of the grid (a quarter, or an eighth for bars like 7/8).
    pub ppq: u32,
    /// Where the take ends, in ticks.
    pub length: i64,
    pub beats_per_bar: u32,
    pub first_downbeat: i64,
    pub aligned: bool,
    /// One part: what was played (start, length, key, MIDI 1.0 velocity for the bench; the 16-bit ones are in
    /// [`Check::velocities`]).
    pub parts: Vec<TakePart>,
    /// Each labelled chord: start, end (ticks) and the chord in Harte's syntax ("C:(1,3,5,7,9)", "N" for no chord).
    /// A chord skipped isn't here, and only the stretches here are scored.
    pub chords: Vec<(i64, i64, String)>,
    pub chord_labels: String,
    pub licence: String,
    /// The day it was played: a day's takes share a fold in the bench, so it never learns from one and is tested on
    /// its twin.
    pub group: String,
    pub whole: String,
    pub check: Check,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TakePart {
    pub role: String,
    pub notes: Vec<(i64, i64, u8, u8)>,
}

/// What only Chord check keeps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Check {
    /// The chord reader's version when it was played (`mo_harmony::CHORD_READER_VERSION`).
    pub reader_version: u32,
    /// Counted on the DAW's song (it played), or from the first note.
    pub on_song: bool,
    /// The notes' MIDI 2.0 16-bit velocities (rule 22), in the order of the part's notes.
    pub velocities: Vec<u16>,
    /// Each chord the reader read, in order, and what Solomon said of it.
    pub spans: Vec<CheckSpan>,
}

/// A chord the reader read and what was said of it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CheckSpan {
    pub start: i64,
    pub end: i64,
    /// What the reader showed ("" for no chord): the one fact that can't be worked out again later, what he judged.
    pub read: String,
    /// The same in Harte's syntax, as the bench compares chords ("" for no chord).
    #[serde(default)]
    pub read_harte: String,
    /// How he labelled it: "right", "choice", "typed", "not a chord", "skipped", or "" not yet.
    pub how: String,
    /// The chord he said, by name ("" for none or not yet).
    pub said: String,
}

/// A take file's name for a take started at `id` ("2026-11-13 07-12-03-250").
pub fn file_name(id: &str) -> String {
    format!("{START}{id}{END}")
}

/// Whether `name` is a take whose writing was cut short: `take <id>.json.mo-part`.
pub fn is_half_written(name: &str) -> bool {
    name.strip_suffix(PART)
        .and_then(|n| n.strip_suffix(END))
        .and_then(|n| n.strip_prefix(START))
        .is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit() || c == '-' || c == ' '))
}

/// Saves `take` in `folder` (made if it isn't there), the safe way: written beside it first, read back and checked,
/// then renamed over the take's file, so a crash part way never leaves a broken take. Never on the window's thread.
pub fn save(take: &TakeFile, folder: &Path) -> Result<PathBuf, String> {
    let bad = |what: &str, e: &dyn std::fmt::Display| format!("Couldn't save the take ({what}): {e}");
    std::fs::create_dir_all(folder).map_err(|e| bad("making its folder", &e))?;
    let path = folder.join(file_name(&take.id));
    let part = folder.join(format!("{}{PART}", file_name(&take.id)));
    let text = serde_json::to_string(take).map_err(|e| bad("writing it out", &e))?;
    let written = std::fs::File::create(&part).and_then(|mut f| {
        use std::io::Write;
        f.write_all(text.as_bytes())?;
        // On the disk before it's renamed over the take: a power cut never leaves an empty take in its place.
        f.sync_all()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(bad("writing it", &e));
    }
    let back = std::fs::read_to_string(&part).map_err(|e| bad("reading it back", &e))?;
    if serde_json::from_str::<TakeFile>(&back).ok().as_ref() != Some(take) {
        let _ = std::fs::remove_file(&part);
        return Err("Couldn't save the take: it didn't read back as it was written".to_owned());
    }
    std::fs::rename(&part, &path).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        bad("putting it in place", &e)
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take() -> TakeFile {
        TakeFile {
            source: SOURCE.into(),
            id: "2026-11-13 07-12-03".into(),
            ppq: 960,
            length: 3840,
            beats_per_bar: 4,
            first_downbeat: 0,
            aligned: true,
            parts: vec![TakePart { role: "chords".into(), notes: vec![(0, 3840, 60, 100), (0, 3840, 64, 100)] }],
            chords: vec![(0, 3840, "C:(1,3)".into())],
            chord_labels: "Solomon".into(),
            licence: OWN.into(),
            group: "chord-check/2026-11-13".into(),
            whole: "song".into(),
            check: Check {
                reader_version: 5,
                on_song: false,
                velocities: vec![50_000, 50_000],
                spans: vec![CheckSpan {
                    start: 0,
                    end: 3840,
                    read: "C".into(),
                    read_harte: "C:(1,3,5)".into(),
                    how: "right".into(),
                    said: "C".into(),
                }],
            },
        }
    }

    #[test]
    fn a_take_is_saved_whole_and_reads_back() {
        let folder = std::env::temp_dir().join(format!("mo-chord-check-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let path = save(&take(), &folder).unwrap();
        assert_eq!(path.file_name().unwrap().to_string_lossy(), "take 2026-11-13 07-12-03.json");
        let back: TakeFile = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back, take());
        // Saved again with a label changed: the same file, nothing left beside it.
        let mut changed = take();
        changed.check.spans[0].how = "typed".into();
        save(&changed, &folder).unwrap();
        let names: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["take 2026-11-13 07-12-03.json"]);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn only_a_takes_own_half_written_name_is_tidied() {
        assert!(is_half_written("take 2026-11-13 07-12-03.json.mo-part"));
        assert!(!is_half_written("take 2026-11-13 07-12-03.json"));
        assert!(!is_half_written("take .json.mo-part"));
        assert!(!is_half_written("my take notes.json.mo-part"));
        assert!(!is_half_written("take 2026-11-13 07-12-03.json.mo-part.txt"));
    }

    #[test]
    fn the_bench_reads_its_own_fields() {
        // The bench's fields under the names it reads (tools/theorybench/src/item.rs).
        let json: serde_json::Value = serde_json::to_value(take()).unwrap();
        for field in ["source", "id", "ppq", "length", "beats_per_bar", "first_downbeat", "parts", "chords", "licence"]
        {
            assert!(json.get(field).is_some(), "{field}");
        }
        assert_eq!(json["parts"][0]["notes"][0], serde_json::json!([0, 3840, 60, 100]));
        assert_eq!(json["whole"], "song");
    }
}
