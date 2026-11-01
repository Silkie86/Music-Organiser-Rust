//! The labelled music the bench reads: one JSON object a line, made from a public dataset by a script in `convert/`.
//! Nothing in it comes from Solomon's own files, and the files stay outside the repository.

use std::io::{BufRead, BufReader};
use std::path::Path;

use mo_harmony::Key;
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// One piece: its notes by part, its bar grid, and the keys and chords someone (or some program) labelled.
#[derive(Clone, Debug, Deserialize)]
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
    /// fitted weights may ship ([`Item::may_learn`]) and only measures the rest.
    #[serde(default)]
    pub licence: String,
}

fn four() -> u32 {
    4
}

fn yes() -> bool {
    true
}

/// One part (a track) and its notes: start, length (ticks), MIDI key, velocity (MIDI 1.0's 0 to 127).
#[derive(Clone, Debug, Deserialize)]
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

impl Item {
    /// Ticks in a bar.
    pub fn bar(&self) -> i64 {
        i64::from(self.beats_per_bar.max(1)) * i64::from(self.ppq.max(1))
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

/// Reads every item in a JSON-lines file.
pub fn read(path: &Path) -> Result<Vec<Item>, String> {
    read_lines(path)
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
        ] {
            assert!(!with(no).may_learn(), "{no}");
        }
    }
}
