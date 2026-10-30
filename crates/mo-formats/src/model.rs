//! What a reader finds in a project, the same for every DAW.

use std::collections::{BTreeMap, HashSet};

use chrono::NaiveDateTime;

/// How much of a file's format the reader understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// The format is well understood; everything is read directly.
    Full,
    /// Most of it is read directly; some details may be missing.
    Partial,
    /// A format nobody has worked out; what's shown was found by looking for readable text.
    Heuristic,
    /// The file is known and listed, but what's inside isn't read.
    Catalogue,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PluginKind {
    #[default]
    Unknown,
    Instrument,
    Effect,
    /// Changes notes on their way to an instrument (an arpeggiator).
    NoteEffect,
}

/// A plugin a project uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginRef {
    pub name: String,
    pub vendor: String,
    pub kind: PluginKind,
    /// "VST2", "VST3", "CLAP", "AU", "AAX", "Native" (the DAW's own), or "VST" when it's a VST of no known kind.
    pub format: String,
    pub path: String,
    /// The plugin's own id as the plugin scanner writes it (a VST3 class id, or a VST2's four letters); empty when
    /// the project doesn't say or the reader doesn't read it yet.
    pub id: String,
}

/// Whether a project plays a sample it holds (see [`ProjectInfo::unplayed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleUse {
    Plays,
    /// Its channel or track has no notes or clips.
    NoNotes,
    /// It has notes or clips, but all of them are muted.
    Muted,
}

/// What a reader found in one project.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectInfo {
    pub daw: &'static str,
    pub confidence: Confidence,
    pub tempo: Option<f64>,
    /// The DAW's version that saved it, as the file writes it.
    pub version: String,
    pub title: String,
    pub plugins: Vec<PluginRef>,
    pub samples: Vec<String>,
    pub tracks: Vec<String>,
    /// Outside instruments the project talks to (Bitwig's HW Instrument devices).
    pub hardware: Vec<String>,
    /// Other facts by name ("PPQ", "Channels", "Genre"...).
    pub details: BTreeMap<String, String>,
    /// When the project was first made, as the file itself records it (FL Studio keeps this through every re-save).
    pub created: Option<NaiveDateTime>,
    pub warnings: Vec<String>,
    pub can_extract_midi: bool,
    pub can_extract_presets: bool,
    /// Samples in `samples` this save doesn't play, and why. Only readers that can tell fill it in; a sample not
    /// listed plays, or its reader can't say.
    pub unplayed: BTreeMap<String, SampleUse>,
}

impl ProjectInfo {
    pub fn new(daw: &'static str) -> ProjectInfo {
        ProjectInfo {
            daw,
            confidence: Confidence::Partial,
            tempo: None,
            version: String::new(),
            title: String::new(),
            plugins: Vec::new(),
            samples: Vec::new(),
            tracks: Vec::new(),
            hardware: Vec::new(),
            details: BTreeMap::new(),
            created: None,
            warnings: Vec::new(),
            can_extract_midi: false,
            can_extract_presets: false,
            unplayed: BTreeMap::new(),
        }
    }

    /// Leaves out blank names, a plugin listed twice with the same role, and a sample listed twice (letter case
    /// aside), keeping the first of each.
    pub fn dedupe(mut self) -> ProjectInfo {
        let mut seen = HashSet::new();
        self.plugins.retain(|p| !p.name.trim().is_empty() && seen.insert((p.name.to_lowercase(), p.kind)));
        let mut seen = HashSet::new();
        self.samples.retain(|s| !s.trim().is_empty() && seen.insert(s.to_lowercase()));
        self
    }
}

/// A note, in ticks at its clip's or song's PPQ. Its velocity is MIDI 2.0's (rule 22): see [`velocity`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteEvent {
    pub start: i64,
    pub length: i64,
    /// MIDI key, 0 to 127 (60 is C5 in FL, C4 elsewhere).
    pub key: u8,
    /// MIDI 2.0's 16-bit velocity, 0 to 65535. [`NoteEvent::velocity7`] is MIDI 1.0's.
    pub velocity: u16,
    /// Muted in the piano roll, so it doesn't play (FL Studio).
    pub muted: bool,
    /// Entered in FL's step sequencer: it has no length of its own and FL plays it until its sound ends.
    pub step: bool,
}

impl NoteEvent {
    /// The velocity as MIDI 1.0 has it, 1 to 127: only for a MIDI 1.0 file, port or DAW, at the last step.
    pub fn velocity7(&self) -> u8 {
        velocity::to_7(self.velocity)
    }
}

/// Note velocities in MIDI 2.0's 16 bits (rule 22), and to and from MIDI 1.0's 7.
pub mod velocity {
    /// The loudest velocity.
    pub const MAX: u16 = u16::MAX;

    /// A velocity given as a share of the loudest (Bitwig's 0 to 1, FL's 0 to 128 over 128).
    pub fn from_unit(share: f64) -> u16 {
        (share * MAX as f64).round_ties_even().clamp(0.0, MAX as f64) as u16
    }

    /// A MIDI 1.0 velocity in 16 bits, by MIDI 2.0's rule (the Universal MIDI Packet spec's "min-center-max"
    /// scaling): 0 stays 0, 64 is the middle, 127 is the loudest, and every value comes back from [`to_7`] as it was.
    pub fn from_7(v: u8) -> u16 {
        let v = v.min(127) as u16;
        let mut out = v << 9;
        if v <= 64 {
            return out;
        }
        // Above the middle, the low six bits are repeated down into the new ones.
        let mut repeat = (v & 0x3f) << 3;
        while repeat != 0 {
            out |= repeat;
            repeat >>= 6;
        }
        out
    }

    /// The MIDI 1.0 velocity nearest a 16-bit one, never 0 (which MIDI 1.0 reads as the note ending).
    pub fn to_7(v: u16) -> u8 {
        ((v as f64 * 127.0 / MAX as f64).round_ties_even() as u8).max(1)
    }
}

/// A clip of notes, by track (or channel) name in the order they first appear.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MidiClip {
    pub name: String,
    pub ppq: u16,
    pub tracks: Vec<(String, Vec<NoteEvent>)>,
    /// Where the clip ends, in ticks, when it goes on past its last note (a loop with empty beats at its end).
    pub end: Option<i64>,
}

impl MidiClip {
    /// The track's notes, adding the track if it isn't there yet.
    // `at` is where the track was just found or added.
    #[allow(clippy::indexing_slicing)]
    pub fn track(&mut self, name: &str) -> &mut Vec<NoteEvent> {
        let at = match self.tracks.iter().position(|(n, _)| n == name) {
            Some(at) => at,
            None => {
                self.tracks.push((name.to_owned(), Vec::new()));
                self.tracks.len() - 1
            }
        };
        &mut self.tracks[at].1
    }
}

/// One channel's (FL Studio) or track's notes across the whole song, with what plays them, in song ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiChannel {
    pub name: String,
    /// What to show for what plays it: the sample's file name for a channel that has one, else the plugin.
    pub source: String,
    pub ppq: u16,
    pub notes: Vec<NoteEvent>,
    pub plugin: String,
    /// The channel's sample, in full, when it plays one.
    pub sample_path: String,
    /// The key the sample sounds at its own pitch on, shifted by `pitch_cents`.
    pub root_key: u8,
    pub pitch_cents: i32,
    /// FL Studio: the channel's place in the channel rack, from 0.
    pub channel: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_1_velocities_come_back_from_16_bits_as_they_were() {
        for v in 1..=127u8 {
            assert_eq!(velocity::to_7(velocity::from_7(v)), v);
        }
        assert_eq!((velocity::from_7(0), velocity::from_7(64), velocity::from_7(127)), (0, 32768, 65535));
        assert_eq!(velocity::to_7(0), 1);
        assert_eq!((velocity::from_unit(1.0), velocity::from_unit(1.5), velocity::from_unit(-1.0)), (65535, 65535, 0));
        // FL's 0 to 128 gives the same MIDI 1.0 velocity the C# app gives, through 16 bits.
        for raw in 0..=128u32 {
            let csharp = (raw as f64 * 127.0 / 128.0).round_ties_even().clamp(1.0, 127.0) as u8;
            assert_eq!(velocity::to_7(velocity::from_unit(raw as f64 / 128.0)), csharp, "{raw}");
        }
    }

    #[test]
    fn dedupe_keeps_the_first_of_each_and_drops_blanks() {
        let plugin = |name: &str, kind| PluginRef { name: name.into(), kind, ..PluginRef::default() };
        let mut info = ProjectInfo::new("Test");
        info.plugins = vec![
            plugin("Serum", PluginKind::Instrument),
            plugin("serum", PluginKind::Instrument),
            plugin("Serum", PluginKind::Effect),
            plugin(" ", PluginKind::Effect),
        ];
        info.samples = vec![r"C:\Kick.wav".into(), r"c:\kick.WAV".into(), "".into()];
        let info = info.dedupe();
        assert_eq!(
            info.plugins.iter().map(|p| (p.name.as_str(), p.kind)).collect::<Vec<_>>(),
            [("Serum", PluginKind::Instrument), ("Serum", PluginKind::Effect)]
        );
        assert_eq!(info.samples, [r"C:\Kick.wav"]);
    }
}
