//! The labelled music the bench reads: one JSON object a line, made from a public dataset by a script in `convert/`,
//! or from Solomon's own MIDI packs by `theorybench packs` on his computer ([`OWN`]); or a folder of Chord check's
//! takes, one file each, that he played into the test plugin and labelled ([`read_takes`]). The files stay outside
//! the repository.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use mo_harmony::{Key, Span};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// One piece: its notes by part, its bar grid, and the keys and chords someone (or some program) labelled.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Item {
    pub source: String,
    pub id: String,
    /// Ticks a beat.
    pub ppq: u32,
    /// Where the music ends, in ticks.
    pub length: i64,
    #[serde(default = "four")]
    pub beats_per_bar: u32,
    /// The tick of the first bar line the labels give (the notes before it are a pickup).
    #[serde(default)]
    pub first_downbeat: i64,
    /// Whether the labelled bar lines fall on the MIDI's own grid.
    #[serde(default = "yes")]
    pub aligned: bool,
    pub parts: Vec<Part>,
    /// Key regions: start, end (ticks) and the key as labelled ("Gb:maj", "C:min").
    #[serde(default)]
    pub keys: Vec<(i64, i64, String)>,
    /// Chord regions: start, end (ticks) and the chord in Harte's syntax ("B:maj", "C#:min7/b3", "N").
    #[serde(default)]
    pub chords: Vec<(i64, i64, String)>,
    /// How the key labels were made ("software (from the audio)", "human").
    #[serde(default)]
    pub key_labels: String,
    #[serde(default)]
    pub chord_labels: String,
    /// The licence the notes and labels come under ("MIT", "CC0", "CC BY 4.0"): keyfit learns only from those whose
    /// fitted weights may ship ([`Item::may_learn`]) and only measures the rest. Solomon's own music says [`OWN`].
    #[serde(default)]
    pub licence: String,
    /// Pieces in the same group share a fold: the files of one pack, which one person made alike. Empty for a
    /// dataset's songs, each its own.
    #[serde(default)]
    pub group: String,
    /// How the piece is heard whole: a dataset's song as a song; a pack's part as the MIDI page hears it.
    #[serde(default)]
    pub whole: Whole,
    /// For a piece `convert/voice.py` played: each labelled chord's start, end and tags (its voicing set, recipe, bass
    /// texture, tune, push, the chart's own label and the quality played), for `chords --by`. Empty for the rest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub render: Vec<(i64, i64, BTreeMap<String, String>)>,
}

/// How a piece is heard whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Whole {
    /// A song, heard as a song.
    #[default]
    Song,
    /// A part of a pack's file, or a section of one between rests, heard as a loop.
    Loop,
    /// A piece the MIDI page cut from a part longer than 16 bars: heard as a window, since it needn't start or end
    /// where the music does.
    Window,
}

impl Whole {
    /// How mo-harmony hears a piece heard this way.
    pub fn span(self) -> Span {
        match self {
            Whole::Song => Span::Song,
            Whole::Loop => Span::Loop,
            Whole::Window => Span::Window,
        }
    }
}

/// The licence `theorybench packs` gives Solomon's own music. Rule 5: only counts and averages from it go in code,
/// and, with his leave (2026-11-01), the key finder's stream weights, minor weight and calibration fitted again
/// (`keyfit --rescale`). Nothing else is ever learned from it.
pub const OWN: &str = "Solomon's own (rule 5)";

fn four() -> u32 {
    4
}

fn yes() -> bool {
    true
}

/// One part (a track) and its notes: start, length (ticks), MIDI key, velocity (MIDI 1.0's 0 to 127).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Part {
    /// "melody", "bass", "chords", "accompaniment", "drums", or "" when the dataset doesn't say.
    #[serde(default)]
    pub role: String,
    pub notes: Vec<(i64, i64, u8, u8)>,
}

/// A note as the bench hands it to an engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub start: i64,
    pub length: i64,
    pub key: u8,
    pub velocity: u8,
}

/// A stretch of a piece needs this many notes to count: in its tune, and in its accompaniment (or any other part).
/// The same for every measure that cuts pieces into stretches (`pairs`, `octave`), so they count the same ones.
pub const LEAST_TUNE: usize = 8;
pub const LEAST_BACKING: usize = 16;

impl Item {
    /// Ticks in a bar.
    pub fn bar(&self) -> i64 {
        i64::from(self.beats_per_bar.max(1)) * i64::from(self.ppq.max(1))
    }

    /// Where its stretches of `bars` bars start, one after another from its first bar line, each wholly within the
    /// piece: how `pairs` and `octave` cut one up ([`cut`] gives a stretch's notes).
    pub fn stretch_starts(&self, bars: i64) -> impl Iterator<Item = i64> + '_ {
        let length = bars * self.bar();
        let origin = self.grid_origin();
        (0..).map(move |n| origin + n * length).take_while(move |from| length > 0 && from + length <= self.length)
    }

    /// The notes of the parts whose role passes `keep`, in order of start.
    pub fn notes_of(&self, keep: impl Fn(&str) -> bool) -> Vec<Note> {
        let mut notes: Vec<Note> = self
            .parts
            .iter()
            .filter(|p| keep(&p.role))
            .flat_map(|p| p.notes.iter())
            .map(|&(start, length, key, velocity)| Note { start, length, key, velocity })
            .collect();
        notes.sort_by_key(|n| (n.start, n.key));
        notes
    }

    /// Where the bar grid starts: the first labelled bar line, taken back whole bars to the first one at or after
    /// tick 0 (the C# bench's `shift`).
    pub fn grid_origin(&self) -> i64 {
        self.first_downbeat.rem_euclid(self.bar())
    }

    /// The key labelled at a tick, if the label reads as a key.
    pub fn key_at(&self, tick: i64) -> Option<Key> {
        self.keys.iter().find(|(s, e, _)| *s <= tick && tick < *e).and_then(|(_, _, k)| Key::parse(k))
    }

    /// The key labelled for the longest time, when the labels read.
    pub fn main_key(&self) -> Option<Key> {
        let mut time = std::collections::HashMap::new();
        for (s, e, k) in &self.keys {
            if let Some(key) = Key::parse(k) {
                *time.entry(key).or_insert(0i64) += e - s;
            }
        }
        time.into_iter().max_by_key(|&(k, t)| (t, std::cmp::Reverse(k.index()))).map(|(k, _)| k)
    }

    /// Whether weights fitted on this piece may ship in the app: its licence is MIT, CC0 or CC BY, without
    /// NonCommercial, ShareAlike or NoDerivatives terms. Anything else, or no licence given, is only measured.
    pub fn may_learn(&self) -> bool {
        let licence = self.licence.to_ascii_uppercase().replace(['-', '_'], " ");
        let words: Vec<&str> = licence.split_whitespace().collect();
        match words.as_slice() {
            ["MIT"] | ["MIT", "LICENSE" | "LICENCE"] => true,
            [first, ..] if first.starts_with("CC0") => true,
            ["CC", "BY", rest @ ..] => !rest.iter().any(|w| matches!(*w, "NC" | "SA" | "ND")),
            _ => false,
        }
    }

    /// Whether this is Solomon's own music ([`OWN`]).
    pub fn is_own(&self) -> bool {
        self.licence == OWN
    }

    /// Whether every key label is the same key (no key change).
    pub fn one_key(&self) -> bool {
        let mut keys = self.keys.iter().filter_map(|(_, _, k)| Key::parse(k));
        match keys.next() {
            Some(first) => keys.all(|k| k == first),
            None => false,
        }
    }
}

/// The notes starting in a stretch, moved to start at 0 and cut off at its end, as the C# bench's `Cut` did. `notes`
/// must be in order of start.
pub fn cut(notes: &[Note], from: i64, length: i64) -> Vec<Note> {
    let first = notes.partition_point(|n| n.start < from);
    let end = notes.partition_point(|n| n.start < from + length);
    notes
        .get(first..end)
        .unwrap_or_default()
        .iter()
        .map(|n| Note { start: n.start - from, length: n.length.min(from + length - n.start).max(1), ..*n })
        .collect()
}

/// Where Chord check's takes come from (`mo_app::chord_check::SOURCE`).
pub const CHORD_CHECK: &str = "chord-check";

/// Reads every item in a JSON-lines file, or every take in a folder of Chord check's ([`read_takes`]).
pub fn read(path: &Path) -> Result<Vec<Item>, String> {
    if path.is_dir() { read_takes(path) } else { read_lines(path) }
}

/// Every take in a folder of Chord check's (the data folder's `Chord check`): each `take <when>.json` is one piece
/// (a half-written `.mo-part` is left alone), in order of when it was played. Only the chords he labelled are scored:
/// a stretch with no label (skipped, or not labelled yet) is marked unknown ("X"), which mir_eval's scores leave out.
/// They're his own (rule 5, [`OWN`]): measured on, never learned from ([`Item::may_learn`]). A take that won't read
/// (cut short, or from a plugin that wrote it otherwise), or isn't marked his own, is left out with a warning, so one
/// bad file never stops the rest being scored.
pub fn read_takes(folder: &Path) -> Result<Vec<Item>, String> {
    let names = std::fs::read_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let mut paths: Vec<std::path::PathBuf> = names
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            name.starts_with("take ") && name.ends_with(".json") && p.is_file()
        })
        .collect();
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let read = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str::<Item>(&text).map_err(|e| e.to_string()))
            .and_then(|item| {
                if item.licence == OWN {
                    Ok(item)
                } else {
                    Err(format!("a Chord check take should be marked \"{OWN}\""))
                }
            });
        match read {
            Ok(mut item) => {
                item.chords = unknown_between(std::mem::take(&mut item.chords), item.length);
                out.push(item);
            }
            Err(why) => eprintln!("{}: left out ({why})", path.display()),
        }
    }
    Ok(out)
}

/// `chords` (in order) with every stretch between them, and before and after them up to `length`, marked unknown.
fn unknown_between(mut chords: Vec<(i64, i64, String)>, length: i64) -> Vec<(i64, i64, String)> {
    chords.sort_by_key(|c| c.0);
    let mut out = Vec::with_capacity(chords.len() * 2 + 1);
    let mut at = 0;
    for (start, end, label) in chords {
        if start > at {
            out.push((at, start, "X".to_owned()));
        }
        at = at.max(end);
        out.push((start, end, label));
    }
    if length > at && !out.is_empty() {
        out.push((at, length, "X".to_owned()));
    }
    out
}

/// Reads a file of one JSON object a line (blank lines skipped); an error names the file and the line.
pub fn read_lines<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (n, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line).map_err(|e| format!("{} line {}: {e}", path.display(), n + 1))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_of_takes_is_read_with_only_the_chords_labelled_scored() {
        /// The test's folder, removed when the test ends, passed or failed.
        struct Gone(std::path::PathBuf);
        impl Drop for Gone {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let folder = std::env::temp_dir().join(format!("theorybench-takes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let _gone = Gone(folder.clone());
        // As the plugin writes it (mo_app::chord_check::TakeFile): the bench's fields and its own besides.
        let take = r#"{"source":"chord-check","id":"2026-11-13 07-12-03-250","ppq":960,"length":7680,
            "beats_per_bar":4,"first_downbeat":0,"aligned":true,
            "parts":[{"role":"chords","notes":[[0,3840,60,100],[0,3840,64,100],[3840,3840,62,90]]}],
            "chords":[[3840,7680,"D:maj"]],"chord_labels":"Solomon","licence":"Solomon's own (rule 5)",
            "group":"chord-check/2026-11-13","whole":"song","check":{"reader_version":5,"spans":[]}}"#;
        std::fs::write(folder.join("take 2026-11-13 07-12-03-250.json"), take).unwrap();
        std::fs::write(folder.join("take 2026-11-13 07-15-00-000.json.mo-part"), "half").unwrap();
        std::fs::write(folder.join("notes.txt"), "his").unwrap();
        std::fs::write(folder.join("take 2026-11-13 07-20-00-000.json"), "{\"cut short").unwrap();
        let items = read(&folder).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.source, CHORD_CHECK);
        assert!(!item.may_learn() && item.is_own(), "rule 5: measured on, never learned from");
        assert_eq!(item.chords, [(0, 3840, "X".to_owned()), (3840, 7680, "D:maj".to_owned())]);
    }

    #[test]
    fn the_stretches_between_labels_are_unknown() {
        let l = |s, e, t: &str| (s, e, t.to_owned());
        let got = unknown_between(vec![l(100, 200, "C:maj"), l(300, 400, "N")], 500);
        assert_eq!(got, [l(0, 100, "X"), l(100, 200, "C:maj"), l(200, 300, "X"), l(300, 400, "N"), l(400, 500, "X")]);
        assert!(unknown_between(Vec::new(), 500).is_empty(), "nothing labelled: nothing scored");
    }

    #[test]
    fn only_licences_that_let_fitted_weights_ship_are_learned_from() {
        let bare: Item = serde_json::from_str(r#"{"source":"x","id":"1","ppq":96,"length":96,"parts":[]}"#).unwrap();
        let with = |licence: &str| Item { licence: licence.to_owned(), ..bare.clone() };
        for yes in ["MIT", "MIT License", "mit licence", "CC0", "CC0-1.0", "CC BY 4.0", "CC-BY-4.0", "cc by"] {
            assert!(with(yes).may_learn(), "{yes}");
        }
        for no in [
            "",
            "CC BY-NC-SA 4.0",
            "CC BY-SA 4.0",
            "CC BY-ND 4.0",
            "ODbL",
            "GPL-3.0",
            "MIT-0 or GPL",
            "measuring only (RS200)",
            OWN,
        ] {
            assert!(!with(no).may_learn(), "{no}");
        }
        assert!(with(OWN).is_own() && !with("MIT").is_own());
    }
}
