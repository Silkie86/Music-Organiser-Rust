//! What instrument a MIDI part is for, but only where it's plain from what comes with the notes (the C#
//! `Instruments`): the track or clip name, a plugin everyone knows (Keyscape is a piano, Addictive Drums is drums),
//! the file and folder names, MIDI channel 10 (General MIDI keeps it for drums) or the General MIDI program a `.mid`
//! file asks for. The notes alone can't say whether chords are for a piano or a pad, so when nothing says, it says
//! nothing.

use std::sync::LazyLock;

use regex::Regex;

/// An instrument family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Piano,
    Organ,
    Bells,
    Guitar,
    Bass,
    Strings,
    Brass,
    Woodwind,
    Vocals,
    Lead,
    Pad,
    Pluck,
    Drums,
    Percussion,
    World,
    Fx,
}

impl Family {
    /// Every family, in the order the page offers them.
    pub const ALL: [Family; 16] = [
        Family::Piano,
        Family::Organ,
        Family::Bells,
        Family::Guitar,
        Family::Bass,
        Family::Strings,
        Family::Brass,
        Family::Woodwind,
        Family::Vocals,
        Family::Lead,
        Family::Pad,
        Family::Pluck,
        Family::Drums,
        Family::Percussion,
        Family::World,
        Family::Fx,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Family::Piano => "Piano",
            Family::Organ => "Organ",
            Family::Bells => "Bells",
            Family::Guitar => "Guitar",
            Family::Bass => "Bass",
            Family::Strings => "Strings",
            Family::Brass => "Brass",
            Family::Woodwind => "Woodwind",
            Family::Vocals => "Vocals",
            Family::Lead => "Lead",
            Family::Pad => "Pad",
            Family::Pluck => "Pluck",
            Family::Drums => "Drums",
            Family::Percussion => "Percussion",
            Family::World => "World",
            Family::Fx => "FX",
        }
    }

    /// Words that search should also find the family by ("keys" finds pianos).
    pub fn search_words(self) -> &'static str {
        match self {
            Family::Piano => "piano keys",
            Family::Bells => "bells mallets",
            Family::Vocals => "vocals vocal vox voice",
            Family::Drums => "drums drum",
            Family::Strings => "strings string orchestra",
            Family::Brass => "brass horns",
            Family::Woodwind => "woodwind wind",
            Family::Organ => "organ",
            Family::Guitar => "guitar",
            Family::Bass => "bass",
            Family::Lead => "lead",
            Family::Pad => "pad",
            Family::Pluck => "pluck",
            Family::Percussion => "percussion",
            Family::World => "world",
            Family::Fx => "fx",
        }
    }

    fn article(self) -> &'static str {
        if matches!(self, Family::Organ | Family::Fx) { "an" } else { "a" }
    }

    /// The family as a word before "plugin": "drum plugin", "piano plugin".
    fn noun(self) -> String {
        match self {
            Family::Drums => "drum".into(),
            Family::Strings => "string".into(),
            Family::Vocals => "vocal".into(),
            Family::Bells => "bell".into(),
            Family::Fx => "effects".into(),
            other => other.name().to_lowercase(),
        }
    }

    /// The family General MIDI puts a program in (programs counted from 0).
    pub fn of_program(program: u8) -> Family {
        match program / 8 {
            0 => Family::Piano,
            1 => Family::Bells,
            2 => Family::Organ,
            3 => Family::Guitar,
            4 => Family::Bass,
            5 => Family::Strings,
            6 if (52..=54).contains(&program) => Family::Vocals,
            6 => Family::Strings,
            7 => Family::Brass,
            8 | 9 => Family::Woodwind,
            10 => Family::Lead,
            11 => Family::Pad,
            12 => Family::Fx,
            13 => Family::World,
            14 => Family::Percussion,
            _ => Family::Fx,
        }
    }
}

/// The instrument a part is for, and how the app knows ("Keyscape is a piano plugin").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentGuess {
    pub family: Family,
    pub why: String,
}

// Plugins and instruments whose name says what they play. Checked first, on whole words (as `simple` writes them, so
// "VB-3" is "vb 3"). Names that also come before other instruments ("upright bass", "grand" anything) aren't here: the
// instrument words below name those.
const PLUGINS: &[(&str, Family)] = &[
    ("addictive drums", Family::Drums),
    ("superior drummer", Family::Drums),
    ("ezdrummer", Family::Drums),
    ("battery", Family::Drums),
    ("stylus", Family::Drums),
    ("drum machine", Family::Drums),
    ("drum rack", Family::Drums),
    ("fpc", Family::Drums),
    ("geist", Family::Drums),
    ("xo", Family::Drums),
    ("analog rytm", Family::Drums),
    ("tr 808", Family::Drums),
    ("tr 909", Family::Drums),
    ("909", Family::Drums),
    ("bass drum", Family::Drums),
    ("cow bell", Family::Drums),
    ("kick", Family::Drums),
    ("snare", Family::Drums),
    ("keyscape", Family::Piano),
    ("pianoteq", Family::Piano),
    ("ravenscroft", Family::Piano),
    ("noire", Family::Piano),
    ("una corda", Family::Piano),
    ("the giant", Family::Piano),
    ("scarbee", Family::Piano),
    ("lounge lizard", Family::Piano),
    ("mark i", Family::Piano),
    ("fazioli", Family::Piano),
    ("steinway", Family::Piano),
    ("rhodes", Family::Piano),
    ("wurlitzer", Family::Piano),
    ("wurli", Family::Piano),
    ("clavinet", Family::Piano),
    ("clav", Family::Piano),
    ("electric piano", Family::Piano),
    ("e piano", Family::Piano),
    ("epiano", Family::Piano),
    ("trilian", Family::Bass),
    ("vb 3", Family::Organ),
    ("hammond", Family::Organ),
    ("farfisa", Family::Organ),
];

/// The plugin table's entries that are words for a sound, not a plugin's name.
const NOT_PLUGIN_NAMES: &[&str] = &[
    "bass drum",
    "kick",
    "snare",
    "electric piano",
    "e piano",
    "epiano",
    "rhodes",
    "wurlitzer",
    "wurli",
    "clavinet",
    "clav",
    "hammond",
    "909",
    "tr 808",
    "tr 909",
    "cow bell",
];

// Libraries mostly known for one family that hold others too ("Spitfire LABS Soft Piano"): only when no word says.
const LIBRARIES: &[(&str, Family)] =
    &[("spitfire", Family::Strings), ("bbc so", Family::Strings), ("albion", Family::Strings)];

// Styles of music whose names hold instrument words: a "Drum and Bass" folder isn't drums, nor "Future Bass" a bass.
const STYLES: &[&str] = &[
    "drum and bass",
    "drum n bass",
    "drum bass",
    "drumnbass",
    "future bass",
    "bass house",
    "bass music",
    "piano house",
];

// Words in names that say what plays. Drum words come before "bass" so "bass drum" is drums; "808" is a bass in names.
const WORDS: &[(&[&str], Family)] = &[
    (
        &[
            "drum",
            "drums",
            "hat",
            "hats",
            "hihat",
            "hihats",
            "hh",
            "ohh",
            "chh",
            "clap",
            "claps",
            "perc",
            "percs",
            "percussion",
            "cymbal",
            "crash",
            "ride",
            "tom",
            "toms",
            "rim",
            "shaker",
            "tambourine",
            "conga",
            "congas",
            "bongo",
            "bongos",
            "cowbell",
        ],
        Family::Drums,
    ),
    (&["piano", "pianos", "keys", "keyboard", "keyboards"], Family::Piano),
    (&["organ", "organs"], Family::Organ),
    (
        &[
            "bell",
            "bells",
            "celesta",
            "glock",
            "glockenspiel",
            "marimba",
            "vibes",
            "vibraphone",
            "xylophone",
            "kalimba",
            "mallet",
            "mallets",
            "music box",
        ],
        Family::Bells,
    ),
    (&["guitar", "guitars", "gtr", "strat", "telecaster"], Family::Guitar),
    (&["bass", "basses", "bassline", "basslines", "sub", "808", "reese"], Family::Bass),
    (
        &[
            "string",
            "strings",
            "violin",
            "violins",
            "viola",
            "cello",
            "cellos",
            "orchestra",
            "orchestral",
            "pizz",
            "pizzicato",
            "harp",
        ],
        Family::Strings,
    ),
    (&["brass", "horn", "horns", "trumpet", "trumpets", "trombone", "tuba"], Family::Brass),
    (
        &["sax", "saxophone", "flute", "flutes", "clarinet", "oboe", "bassoon", "recorder", "pan flute"],
        Family::Woodwind,
    ),
    (&["vox", "vocal", "vocals", "voice", "voices", "choir", "chant"], Family::Vocals),
    (&["lead", "leads"], Family::Lead),
    (&["pad", "pads", "atmos", "atmosphere"], Family::Pad),
    (&["pluck", "plucks"], Family::Pluck),
];

/// Each table's words with a space either side, made once: names are checked for every part on every catalogue load.
struct Tables {
    styles: Vec<String>,
    plugins: Vec<(String, &'static str, Family)>,
    words: Vec<(String, &'static str, Family)>,
    libraries: Vec<(String, Family)>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    let padded = |w: &str| format!(" {w} ");
    Tables {
        styles: STYLES.iter().map(|s| padded(s)).collect(),
        plugins: PLUGINS.iter().map(|&(w, f)| (padded(w), w, f)).collect(),
        words: WORDS.iter().flat_map(|&(ws, f)| ws.iter().map(move |&w| (padded(w), w, f))).collect(),
        libraries: LIBRARIES.iter().map(|&(w, f)| (padded(w), f)).collect(),
    }
});

/// The instrument, when something says. `names` are the track name, clip name, file name and the folders (most
/// specific first: the first that says wins); `channel10` is a `.mid` part on MIDI channel 10; `program` the General
/// MIDI program a `.mid` file sets (0 to 127).
pub fn guess<'a>(
    names: impl IntoIterator<Item = &'a str>,
    channel10: bool,
    program: Option<u8>,
) -> Option<InstrumentGuess> {
    if channel10 {
        return Some(InstrumentGuess {
            family: Family::Drums,
            why: "It's on MIDI channel 10, which General MIDI keeps for drums".into(),
        });
    }
    let t = &*TABLES;
    for raw in names {
        if raw.trim().is_empty() {
            continue;
        }
        let shown = raw.trim();
        let mut text = format!(" {} ", simple(raw));
        for style in &t.styles {
            if text.contains(style.as_str()) {
                text = text.replace(style.as_str(), " ");
            }
        }
        for (padded, words, family) in &t.plugins {
            if text.contains(padded.as_str()) {
                let why = if NOT_PLUGIN_NAMES.contains(words) {
                    format!("The name “{shown}” says {words}")
                } else {
                    format!("“{shown}” is {} {} plugin", family.article(), family.noun())
                };
                return Some(InstrumentGuess { family: *family, why });
            }
        }
        for (padded, word, family) in &t.words {
            if text.contains(padded.as_str()) {
                return Some(InstrumentGuess { family: *family, why: format!("The name “{shown}” says {word}") });
            }
        }
        for (padded, family) in &t.libraries {
            if text.contains(padded.as_str()) {
                return Some(InstrumentGuess {
                    family: *family,
                    why: format!("“{shown}” is mostly {} {} library", family.article(), family.noun()),
                });
            }
        }
    }
    // Program 0 (Acoustic Grand Piano) is what most programs write when nobody chose a sound, so it says nothing.
    let program = program.filter(|p| (1..128).contains(p))?;
    Some(InstrumentGuess {
        family: Family::of_program(program),
        why: format!("The file asks for General MIDI sound {}: {}", program as u32 + 1, general_midi(program)),
    })
}

/// Lower case, with anything that isn't a letter or digit as a space, and letters and digits split ("Pad2" →
/// "pad 2"), as the C# `Simple` writes names.
fn simple(s: &str) -> String {
    static NOT_WORD: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}]+").ok());
    static LETTER_DIGIT: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(\p{L})(\p{N})").ok());
    let lowered = mo_formats::case::to_lower(s);
    let spaced = match NOT_WORD.as_ref() {
        Some(re) => re.replace_all(&lowered, " ").into_owned(),
        None => lowered,
    };
    let split = match LETTER_DIGIT.as_ref() {
        Some(re) => re.replace_all(&spaced, "$1 $2").into_owned(),
        None => spaced,
    };
    split.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The General MIDI sound with this program number (counted from 0); "" past 127.
pub fn general_midi(program: u8) -> &'static str {
    GENERAL_MIDI.get(program as usize).copied().unwrap_or("")
}

/// The 128 General MIDI sounds, by program number from 0.
const GENERAL_MIDI: [&str; 128] = [
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
    "Acoustic Guitar (nylon)",
    "Acoustic Guitar (steel)",
    "Electric Guitar (jazz)",
    "Electric Guitar (clean)",
    "Electric Guitar (muted)",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Electric Bass (finger)",
    "Electric Bass (pick)",
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
    "Lead 1 (square)",
    "Lead 2 (sawtooth)",
    "Lead 3 (calliope)",
    "Lead 4 (chiff)",
    "Lead 5 (charang)",
    "Lead 6 (voice)",
    "Lead 7 (fifths)",
    "Lead 8 (bass + lead)",
    "Pad 1 (new age)",
    "Pad 2 (warm)",
    "Pad 3 (polysynth)",
    "Pad 4 (choir)",
    "Pad 5 (bowed)",
    "Pad 6 (metallic)",
    "Pad 7 (halo)",
    "Pad 8 (sweep)",
    "FX 1 (rain)",
    "FX 2 (soundtrack)",
    "FX 3 (crystal)",
    "FX 4 (atmosphere)",
    "FX 5 (brightness)",
    "FX 6 (goblins)",
    "FX 7 (echoes)",
    "FX 8 (sci-fi)",
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

    fn family(names: &[&str]) -> Option<Family> {
        guess(names.iter().copied(), false, None).map(|g| g.family)
    }

    #[test]
    fn names_say_the_instrument() {
        assert_eq!(family(&["Keyscape"]), Some(Family::Piano));
        assert_eq!(guess(["Keyscape"], false, None).unwrap().why, "“Keyscape” is a piano plugin");
        assert_eq!(family(&["VB-3 II"]), Some(Family::Organ));
        assert_eq!(family(&["Bass drum 2"]), Some(Family::Drums));
        assert_eq!(family(&["Pad2"]), Some(Family::Pad));
        assert_eq!(family(&["Big 808"]), Some(Family::Bass));
        assert_eq!(family(&["Spitfire LABS Soft Piano"]), Some(Family::Piano));
        assert_eq!(family(&["Spitfire LABS"]), Some(Family::Strings));
        // Styles of music aren't instruments.
        assert_eq!(family(&["Drum and Bass"]), None);
        assert_eq!(family(&["Future Bass Chords"]), None);
        // The first name that says wins.
        assert_eq!(family(&["", "Lead", "Piano"]), Some(Family::Lead));
        assert_eq!(family(&["Pattern 3"]), None);
        // Whole words only.
        assert_eq!(family(&["Subtle"]), None);
    }

    #[test]
    fn channel_ten_and_programs() {
        assert_eq!(guess(["Piano"], true, None).unwrap().family, Family::Drums);
        let g = guess(["Track 1"], false, Some(38)).unwrap();
        assert_eq!(g.family, Family::Bass);
        assert_eq!(g.why, "The file asks for General MIDI sound 39: Synth Bass 1");
        // Program 0 is what files write when nobody chose a sound.
        assert_eq!(guess(["Track 1"], false, Some(0)), None);
        assert_eq!(Family::of_program(53), Family::Vocals);
        assert_eq!(Family::of_program(48), Family::Strings);
        assert_eq!(Family::of_program(127), Family::Fx);
        assert_eq!(general_midi(127), "Gunshot");
    }
}
