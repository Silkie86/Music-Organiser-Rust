//! Which General MIDI sound a part plays with when the MIDI page plays it (Solomon, 2026-11-05: "play midi as a loop
//! using a general midi library where the sound is matched automatically"). The match goes, first to last:
//!
//! 1. A drum part (channel 10, or the kind finder says drums) plays the GM drum kit.
//! 2. A MIDI file's own program change: its maker said which instrument it's for.
//! 3. Words in the track's, part's or file's name ("Rhodes", "808", "Warm pad", "Strings").
//! 4. The instrument the page's instrument finder names (a plugin everyone knows: Keyscape is a piano).
//! 5. The kind of part: chords on an electric piano, a melody on a saw lead, a bassline on a synth bass, an arp on a
//!    harp (as the C# app's defaults).
//!
//! Nothing here plays: it only says which sound, and why, for the card to show.

use crate::{Family, Role};

/// A sound to play a part with: a General MIDI program (0 to 127), or the drum kit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GmSound {
    /// The GM program, 0 to 127. For the drum kit it's 0 (the standard kit), played on channel 10.
    pub program: u8,
    pub drums: bool,
    /// Why this one: what the card says under the sound's name.
    pub why: Why,
}

/// What a sound was matched by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// A drum part.
    Drums,
    /// The MIDI file's own program change.
    Program,
    /// A word in a name, as the name writes it (in small letters): "harpsichord", not the "harpsi" it was matched by;
    /// for two words that name a sound together, that sound ("synth strings", "e-piano").
    Name(String),
    /// The instrument the instrument finder names.
    Instrument(Family),
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
        match &self.why {
            Why::Drums => "a drum part".to_owned(),
            Why::Program => "the file's own instrument".to_owned(),
            Why::Name(word) => format!("from its name: {word}"),
            Why::Instrument(family) => format!("for its instrument: {}", family.name().to_lowercase()),
            Why::Kind => "for its kind of part".to_owned(),
        }
    }
}

/// The sound for a part: `drums` when it's on channel 10, `program` its file's program change (a project's channel
/// gives none), `names` its track's, part's and file's names, best first, `family` the instrument finder's answer.
pub fn sound_for(role: Role, drums: bool, program: Option<u8>, names: &[&str], family: Option<Family>) -> GmSound {
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
    if let Some(family) = family {
        return match for_family(family) {
            Some(program) => GmSound { program, drums: false, why: Why::Instrument(family) },
            None => GmSound { why: Why::Instrument(family), ..GmSound::DRUM_KIT },
        };
    }
    GmSound { program: for_kind(role), drums: false, why: Why::Kind }
}

/// The sound for an instrument family; `None` for the drum kit.
pub fn for_family(family: Family) -> Option<u8> {
    Some(match family {
        Family::Piano => 0,
        Family::Organ => 16,
        Family::Bells => 11,
        Family::Guitar => 27,
        Family::Bass => 38,
        Family::Strings => 48,
        Family::Brass => 61,
        Family::Woodwind => 73,
        Family::Vocals => 52,
        Family::Lead => 81,
        Family::Pad => 89,
        Family::Pluck => 45,
        Family::World => 108,
        Family::Fx => 99,
        Family::Drums | Family::Percussion => return None,
    })
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

/// Two words that together say more than either alone ("Synth strings" aren't strings, "Acoustic bass" isn't a
/// guitar). Looked for before [`WORDS`].
const PAIRS: &[(&str, &str, u8, &str)] = &[
    ("synth", "string", 50, "synth strings"),
    ("synth", "brass", 62, "synth brass"),
    ("synth", "bass", 38, "synth bass"),
    ("acoustic", "bass", 32, "acoustic bass"),
    ("acoustic", "piano", 0, "acoustic piano"),
    ("bass", "guitar", 33, "bass guitar"),
    ("electric", "piano", 4, "electric piano"),
    ("e", "piano", 4, "e-piano"),
];

/// How a word in [`WORDS`] is matched to a word in a name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Match {
    /// The start of a word: "strings" by "string", "bassline" by "bass", "808s" by "808".
    Start,
    /// The whole word, or it with an "s" ("pads", "subs"): so "subtle", "panic", "organic" and "harpsichord" don't
    /// count as a sub, a pan flute, an organ or a harp.
    Whole,
}

use Match::{Start, Whole};

/// Words in names and the program each stands for, the more particular ones first ("harpsichord" before "harp",
/// "808" and "bass" before "guitar" and "acoustic", "pad" before "synth"). Drum words are left out: a drum part is
/// known by its channel and its kind.
const WORDS: &[(&str, Match, u8)] = &[
    ("rhodes", Start, 4),
    ("wurli", Start, 5),
    ("epiano", Start, 4),
    ("ep", Whole, 4),
    ("clav", Start, 7),
    ("harpsi", Start, 6),
    ("celesta", Start, 8),
    ("glock", Start, 9),
    ("musicbox", Start, 10),
    ("vibe", Whole, 11),
    ("vibra", Start, 11),
    ("marimba", Start, 12),
    ("xylo", Start, 13),
    ("bell", Start, 14),
    ("organ", Whole, 16),
    ("accordion", Start, 21),
    ("harmonica", Start, 22),
    ("808", Start, 38),
    ("reese", Start, 39),
    ("sub", Whole, 38),
    ("fretless", Start, 35),
    ("slap", Start, 36),
    // Before "bass", which it starts with.
    ("bassoon", Start, 70),
    ("bass", Start, 38),
    ("piano", Start, 0),
    ("grand", Whole, 0),
    ("nylon", Start, 24),
    ("guitar", Start, 27),
    ("gtr", Whole, 27),
    ("acoustic", Start, 25),
    ("violin", Start, 40),
    ("viola", Whole, 41),
    ("cello", Start, 42),
    ("pizz", Start, 45),
    ("pluck", Start, 45),
    ("harp", Whole, 46),
    ("timpani", Start, 47),
    ("string", Start, 48),
    ("choir", Start, 52),
    ("vox", Whole, 53),
    ("vocal", Start, 53),
    ("voice", Start, 53),
    ("orch", Start, 55),
    ("hit", Whole, 55),
    ("trumpet", Start, 56),
    ("trombone", Start, 57),
    ("tuba", Whole, 58),
    ("horn", Whole, 60),
    ("brass", Start, 61),
    ("sax", Start, 65),
    ("oboe", Start, 68),
    ("clarinet", Start, 71),
    ("piccolo", Start, 72),
    ("flute", Start, 73),
    ("pan", Whole, 75),
    ("whistle", Start, 78),
    ("ocarina", Start, 79),
    ("square", Start, 80),
    ("saw", Whole, 81),
    ("lead", Whole, 81),
    ("pad", Whole, 89),
    ("warm", Whole, 89),
    ("atmos", Start, 99),
    ("sitar", Start, 104),
    ("kalimba", Start, 108),
    ("steel", Whole, 114),
    ("synth", Start, 81),
    ("keys", Whole, 4),
];

/// Whether `word` (from [`WORDS`]) is the name's word `w`.
fn matches(w: &str, word: &str, how: Match) -> bool {
    // A bassoon isn't a bass, in a pair ("synth bassoon") too.
    if word == "bass" && w.starts_with("bassoon") {
        return false;
    }
    match how {
        Start => w.starts_with(word),
        Whole => w == word || w.strip_prefix(word).is_some_and(|rest| rest == "s" || rest == "es"),
    }
}

/// The sound a name's words stand for, if any word does.
fn by_name(name: &str) -> Option<GmSound> {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for &(a, b, program, said) in PAIRS {
        // "E piano" only as two words side by side: "House piano" isn't an electric piano.
        let found = if a == "e" {
            words.windows(2).any(|p| p[0] == "e" && matches(p[1], b, Start))
        } else {
            words.iter().any(|w| matches(w, a, Start)) && words.iter().any(|w| matches(w, b, Start))
        };
        if found {
            return Some(GmSound { program, drums: false, why: Why::Name(said.to_owned()) });
        }
    }
    for &(word, how, program) in WORDS {
        if let Some(w) = words.iter().find(|w| matches(w, word, how)) {
            return Some(GmSound { program, drums: false, why: Why::Name((*w).to_owned()) });
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
        sound_for(role, false, None, names, None)
    }

    #[test]
    fn drums_play_the_kit_whatever_they_are_called() {
        assert_eq!(sound_for(Role::Chords, true, Some(4), &["Piano"], None), GmSound::DRUM_KIT);
        assert_eq!(named(Role::Drums, &["Rhodes"]).name(), "Drum kit");
    }

    #[test]
    fn a_files_own_program_comes_before_its_name() {
        let s = sound_for(Role::Chords, false, Some(48), &["Piano"], None);
        assert_eq!((s.program, s.why.clone(), s.name()), (48, Why::Program, "String Ensemble 1"));
        // Program 0 is mostly a maker leaving it as it was: the name says more.
        assert_eq!(sound_for(Role::Bass, false, Some(0), &["808 slide"], None).program, 38);
        assert_eq!(sound_for(Role::Bass, false, Some(0), &["Untitled"], None).program, 0);
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
        assert_eq!(named(Role::Chords, &["Organ"]).why, Why::Name("organ".into()));
        // Some words only on their own (or with an s).
        assert_eq!(named(Role::Chords, &["Subtle panic"]).why, Why::Kind);
        assert_eq!(named(Role::Chords, &["Ambient Pads"]).program, 89);
        assert_eq!(named(Role::Bass, &["808s"]).program, 38);
        assert_eq!(named(Role::Melody, &["Tenor Saxophone"]).program, 65);
        assert_eq!(named(Role::Chords, &["Harpsichord"]).program, 6);
        // A bassoon isn't a bass; and the card says the word as the name has it, not the start it's matched by.
        assert_eq!(named(Role::Melody, &["Bassoon"]).program, 70);
        assert_eq!(named(Role::Melody, &["Synth Bassoon"]).program, 70);
        assert_eq!(named(Role::Chords, &["Harpsichord"]).why_text(), "from its name: harpsichord");
        assert_eq!(named(Role::Chords, &["Organic pad"]).program, 89);
        // Two words that say more together, and electric piano only when it says so.
        assert_eq!(named(Role::Chords, &["Synth Strings"]).program, 50);
        assert_eq!(named(Role::Bass, &["Acoustic Bass"]).program, 32);
        assert_eq!(named(Role::Chords, &["Acoustic Grand Piano"]).program, 0);
        assert_eq!(named(Role::Bass, &["Bass Guitar"]).program, 33);
        assert_eq!(named(Role::Chords, &["House piano"]).program, 0);
        assert_eq!(named(Role::Chords, &["E Piano"]).program, 4);
    }

    #[test]
    fn the_instrument_finder_comes_after_names() {
        let keyscape = sound_for(Role::Chords, false, None, &["Pattern 3"], Some(Family::Piano));
        assert_eq!((keyscape.program, keyscape.why_text()), (0, "for its instrument: piano".to_owned()));
        assert_eq!(sound_for(Role::Chords, false, None, &["Warm pad"], Some(Family::Piano)).program, 89);
        assert!(sound_for(Role::Other, false, None, &[], Some(Family::Percussion)).drums);
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
        assert!(WORDS.iter().all(|(_, _, p)| *p < 128));
        assert!(PAIRS.iter().all(|(_, _, p, _)| *p < 128));
    }
}
