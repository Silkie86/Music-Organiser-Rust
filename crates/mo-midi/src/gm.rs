//! Which General MIDI sound a part plays with when the MIDI page plays it (Solomon, 2026-11-05: "play midi as a loop
//! using a general midi library where the sound is matched automatically"). The match goes, first to last:
//!
//! 1. A drum part (channel 10, or the kind finder says drums) plays the GM drum kit.
//! 2. A MIDI file's own program change: its maker said which instrument it's for.
//! 3. Words in the track's, part's or file's name ("Rhodes", "808", "Warm pad", "Strings").
//! 4. The kind of part: chords on an electric piano, a melody on a saw lead, a bassline on a synth bass, an arp on a
//!    harp (as the C# app's defaults).
//!
//! Nothing here plays: it only says which sound, and why, for the card to show.

use crate::Role;

/// A sound to play a part with: a General MIDI program (0 to 127), or the drum kit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GmSound {
    /// The GM program, 0 to 127. For the drum kit it's 0 (the standard kit), played on channel 10.
    pub program: u8,
    pub drums: bool,
    /// Why this one: what the card says under the sound's name.
    pub why: Why,
}

/// What a sound was matched by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// A drum part.
    Drums,
    /// The MIDI file's own program change.
    Program,
    /// A word in a name (the word, as it was matched).
    Name(&'static str),
    /// The kind of part.
    Kind,
}

impl GmSound {
    pub const DRUM_KIT: GmSound = GmSound { program: 0, drums: true, why: Why::Drums };

    /// The sound's name, as the card shows it ("Electric Piano 1", "Drum kit").
    pub fn name(&self) -> &'static str {
        if self.drums { "Drum kit" } else { PROGRAM_NAMES[usize::from(self.program & 0x7f)] }
    }

    /// The words under its name ("from its name: rhodes").
    pub fn why_text(&self) -> String {
        match self.why {
            Why::Drums => "a drum part".to_owned(),
            Why::Program => "the file's own instrument".to_owned(),
            Why::Name(word) => format!("from its name: {word}"),
            Why::Kind => "for its kind of part".to_owned(),
        }
    }
}

/// The sound for a part: `drums` when it's on channel 10, `program` its file's program change (a project's channel
/// gives none), `names` its track's, part's and file's names, best first.
pub fn sound_for(role: Role, drums: bool, program: Option<u8>, names: &[&str]) -> GmSound {
    if drums || role == Role::Drums {
        return GmSound::DRUM_KIT;
    }
    // A file whose maker left the program at 0 (piano) mostly meant nothing by it: a name says more then.
    if let Some(p) = program.filter(|p| *p > 0 && *p < 128) {
        return GmSound { program: p, drums: false, why: Why::Program };
    }
    for name in names {
        if let Some(sound) = by_name(name) {
            return sound;
        }
    }
    if program == Some(0) {
        return GmSound { program: 0, drums: false, why: Why::Program };
    }
    GmSound { program: for_kind(role), drums: false, why: Why::Kind }
}

/// The sound a kind of part starts with (the C# app's `GmSounds.DefaultFor`).
pub fn for_kind(role: Role) -> u8 {
    match role {
        Role::Chords => 4,
        Role::Melody => 81,
        Role::Bass => 38,
        Role::Arp => 46,
        Role::Drums | Role::Other => 0,
    }
}

/// Words in names and the program each stands for. A word matches the start of a word in the name ("strings" by
/// "string", "bassline" by "bass"), so the more particular ones come first ("e-piano" before "piano", "808" before
/// "bass"). Drum words are left out: a drum part is known by its channel and its kind.
const WORDS: &[(&str, u8)] = &[
    ("rhodes", 4),
    ("wurli", 5),
    ("epiano", 4),
    ("ep", 4),
    ("clav", 7),
    ("celesta", 8),
    ("glock", 9),
    ("musicbox", 10),
    ("vibe", 11),
    ("vibra", 11),
    ("marimba", 12),
    ("xylo", 13),
    ("bell", 14),
    ("organ", 16),
    ("accordion", 21),
    ("harmonica", 22),
    ("nylon", 24),
    ("acoustic", 25),
    ("guitar", 27),
    ("gtr", 27),
    ("808", 38),
    ("reese", 39),
    ("sub", 38),
    ("fretless", 35),
    ("slap", 36),
    ("bass", 38),
    ("violin", 40),
    ("viola", 41),
    ("cello", 42),
    ("pizz", 45),
    ("pluck", 45),
    ("harp", 46),
    ("timpani", 47),
    ("string", 48),
    ("choir", 52),
    ("vox", 53),
    ("vocal", 53),
    ("voice", 53),
    ("orch", 55),
    ("hit", 55),
    ("trumpet", 56),
    ("trombone", 57),
    ("tuba", 58),
    ("horn", 60),
    ("brass", 61),
    ("sax", 65),
    ("oboe", 68),
    ("clarinet", 71),
    ("piccolo", 72),
    ("flute", 73),
    ("pan", 75),
    ("whistle", 78),
    ("ocarina", 79),
    ("square", 80),
    ("saw", 81),
    ("lead", 81),
    ("pad", 89),
    ("warm", 89),
    ("atmos", 99),
    ("sitar", 104),
    ("kalimba", 108),
    ("steel", 114),
    ("synth", 81),
    ("keys", 4),
    ("piano", 0),
];

/// The sound a name's words stand for, if any word does.
fn by_name(name: &str) -> Option<GmSound> {
    let lower = name.to_lowercase();
    // "E-Piano", "e piano" and "EPiano" all say electric piano.
    let joined = lower.replace(['-', '_'], "");
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let joined_words: Vec<&str> = joined.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if joined.contains("e piano") || joined_words.iter().any(|w| w.starts_with("epiano")) {
        return Some(GmSound { program: 4, drums: false, why: Why::Name("e-piano") });
    }
    for &(word, program) in WORDS {
        // Short words only as a whole word ("ep", "sub", "pan", "hit"), so "subtle", "panic" and "white" don't count.
        let found = if word.len() <= 3 {
            words.contains(&word)
        } else {
            words.iter().any(|w| w.starts_with(word))
                || (word.chars().all(|c| c.is_ascii_digit()) && lower.contains(word))
        };
        if found {
            return Some(GmSound { program, drums: false, why: Why::Name(word) });
        }
    }
    None
}

/// General MIDI's 128 instrument names, by program.
pub const PROGRAM_NAMES: [&str; 128] = [
    "Acoustic Grand Piano",
    "Bright Acoustic Piano",
    "Electric Grand Piano",
    "Honky-tonk Piano",
    "Electric Piano 1",
    "Electric Piano 2",
    "Harpsichord",
    "Clavinet",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular Bells",
    "Dulcimer",
    "Drawbar Organ",
    "Percussive Organ",
    "Rock Organ",
    "Church Organ",
    "Reed Organ",
    "Accordion",
    "Harmonica",
    "Tango Accordion",
    "Nylon Guitar",
    "Steel Guitar",
    "Jazz Guitar",
    "Clean Guitar",
    "Muted Guitar",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Finger Bass",
    "Picked Bass",
    "Fretless Bass",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Strings",
    "Pizzicato Strings",
    "Orchestral Harp",
    "Timpani",
    "String Ensemble 1",
    "String Ensemble 2",
    "Synth Strings 1",
    "Synth Strings 2",
    "Choir Aahs",
    "Voice Oohs",
    "Synth Voice",
    "Orchestra Hit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "Muted Trumpet",
    "French Horn",
    "Brass Section",
    "Synth Brass 1",
    "Synth Brass 2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Blown Bottle",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Square Lead",
    "Saw Lead",
    "Calliope Lead",
    "Chiff Lead",
    "Charang Lead",
    "Voice Lead",
    "Fifths Lead",
    "Bass and Lead",
    "New Age Pad",
    "Warm Pad",
    "Polysynth Pad",
    "Choir Pad",
    "Bowed Pad",
    "Metallic Pad",
    "Halo Pad",
    "Sweep Pad",
    "Rain",
    "Soundtrack",
    "Crystal",
    "Atmosphere",
    "Brightness",
    "Goblins",
    "Echoes",
    "Sci-fi",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bagpipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko Drum",
    "Melodic Tom",
    "Synth Drum",
    "Reverse Cymbal",
    "Guitar Fret Noise",
    "Breath Noise",
    "Seashore",
    "Bird Tweet",
    "Telephone Ring",
    "Helicopter",
    "Applause",
    "Gunshot",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn named(role: Role, names: &[&str]) -> GmSound {
        sound_for(role, false, None, names)
    }

    #[test]
    fn drums_play_the_kit_whatever_they_are_called() {
        assert_eq!(sound_for(Role::Chords, true, Some(4), &["Piano"]), GmSound::DRUM_KIT);
        assert_eq!(named(Role::Drums, &["Rhodes"]).name(), "Drum kit");
    }

    #[test]
    fn a_files_own_program_comes_before_its_name() {
        let s = sound_for(Role::Chords, false, Some(48), &["Piano"]);
        assert_eq!((s.program, s.why, s.name()), (48, Why::Program, "String Ensemble 1"));
        // Program 0 is mostly a maker leaving it as it was: the name says more.
        assert_eq!(sound_for(Role::Bass, false, Some(0), &["808 slide"]).program, 38);
        assert_eq!(sound_for(Role::Bass, false, Some(0), &["Untitled"]).program, 0);
    }

    #[test]
    fn names_pick_the_sound() {
        assert_eq!(named(Role::Melody, &["Rhodes chords"]).program, 4);
        assert_eq!(named(Role::Melody, &["E-Piano 2"]).program, 4);
        assert_eq!(named(Role::Melody, &["Dark keys 01"]).program, 4);
        assert_eq!(named(Role::Melody, &["Grand Piano"]).program, 0);
        assert_eq!(named(Role::Chords, &["Warm Pad"]).program, 89);
        assert_eq!(named(Role::Chords, &["Synth pad"]).program, 89);
        assert_eq!(named(Role::Melody, &["Synth bass"]).program, 38);
        assert_eq!(named(Role::Chords, &["Lush strings"]).program, 48);
        assert_eq!(named(Role::Melody, &["808 slide 03"]).program, 38);
        assert_eq!(named(Role::Melody, &["Bassline"]).program, 38);
        // The first name with a word in it wins: the track's before the file's.
        assert_eq!(named(Role::Chords, &["Organ", "Night drive pad"]).program, 16);
        assert_eq!(named(Role::Chords, &["Organ"]).why, Why::Name("organ"));
        // Short words only on their own.
        assert_eq!(named(Role::Chords, &["Subtle panic"]).why, Why::Kind);
    }

    #[test]
    fn the_kind_decides_when_nothing_else_does() {
        assert_eq!(named(Role::Chords, &["Pattern 3"]).program, 4);
        assert_eq!(named(Role::Melody, &[]).program, 81);
        assert_eq!(named(Role::Bass, &["Pattern 1"]).program, 38);
        assert_eq!(named(Role::Arp, &["Pattern 2"]).program, 46);
        assert_eq!(named(Role::Other, &["x"]).name(), "Acoustic Grand Piano");
    }

    #[test]
    fn every_program_has_a_name() {
        assert!(PROGRAM_NAMES.iter().all(|n| !n.is_empty()));
        assert!(WORDS.iter().all(|(_, p)| *p < 128));
    }
}
