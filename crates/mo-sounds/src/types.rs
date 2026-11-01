//! The kinds of sound the app sorts samples into, and the clues in a sample's folder and file name used as starting
//! labels before the classifier has learned your library. A port of the C# `Sounds/SoundTypes.cs`, rule for rule.

use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use crate::{file_name, music, stem};

/// The type of a sound nothing has named yet.
pub const UNKNOWN: &str = "Unknown";

/// (type, pattern over the lower-cased name, prompt for the CLAP model). Order matters: earlier, more specific types win.
const RULES: [(&str, &str, &str); 23] = [
    ("808", r"\b808s?\b|\bsub ?bass|\bsubs?\b", "an 808 sub bass"),
    ("Kick", r"\bkicks?\b|\bkik\b|\bbd\b|bassdrum|bass drum|\bkck", "a kick drum"),
    ("Snare", r"\bsnares?\b|\bsnr\b|\bsd\b|rimshot|\brim\b", "a snare drum"),
    ("Clap", r"\bclaps?\b|\bclp\b|handclap|\bsnap(s)?\b", "a hand clap"),
    ("Closed hat", r"closed ?(hi ?-?)?hats?|\bchh?\b|\bc ?hat\b", "a closed hi-hat"),
    ("Open hat", r"open ?(hi ?-?)?hats?|\bohh?\b|\bo ?hat\b", "an open hi-hat"),
    ("Hi-hat", r"\bhi ?-?hats?\b|\bhats?\b|\bhh\b", "a hi-hat"),
    ("Cymbal", r"\bcymbals?\b|\bcrash(es)?\b|\brides?\b|\bsplash\b|\bchina\b", "a cymbal crash"),
    ("Tom", r"\btoms?\b|\bfloor ?tom", "a tom drum"),
    (
        "Percussion",
        r"\bperc(ussion)?s?\b|\bshakers?\b|\bconga|\bbongo|tambourine|\bcowbell|\bclave|\bwoodblock|\btriangle|\bblock\b",
        "a percussion hit",
    ),
    (
        "Drum loop",
        r"\bdrum ?loops?\b|\bbeats?\b|\bbreaks?\b|\bbreakbeats?\b|\btop ?loops?\b|\bgrooves?\b|\bdrums?\b.*\bloop",
        "a drum loop",
    ),
    (
        "Vocal",
        r"\bvox\b|\bvocals?\b|\bvoice|\bacapella|\bchops?\b|\bphrase|\bspoken|\bchants?\b|\bad ?libs?\b",
        "a vocal",
    ),
    ("Bass", r"\bbass(es|line)?\b|\breese\b", "a bass synth"),
    ("Pad", r"\bpads?\b|\bdrones?\b", "a synth pad"),
    ("Lead", r"\bleads?\b|\bsynth ?lines?\b", "a synth lead"),
    ("Pluck", r"\bplucks?\b|\bpizz", "a pluck"),
    ("Keys", r"\bkeys\b|\bpiano|\brhodes\b|\bepiano|\borgan\b|\bwurli", "piano keys"),
    ("Chord", r"\bchords?\b|\bstabs?\b|\bprogression", "a chord stab"),
    ("Guitar", r"\bguitars?\b|\bgtr\b", "a guitar"),
    ("Strings & brass", r"\bstrings?\b|\bviolin|\bcello|\bbrass\b|\bhorns?\b|\btrumpet|\bsax", "strings or brass"),
    (
        "FX",
        r"\bfx\b|\bsfx\b|\brisers?\b|\bdownlifters?\b|\buplifters?\b|\bsweeps?\b|\bimpacts?\b|\bwhoosh|\btransitions?\b|\bnoise\b|\bzaps?\b|\breverse",
        "a sound effect riser or impact",
    ),
    ("Texture", r"\btextures?\b|\batmos(phere)?s?\b|\bambien(ce|t)\b|\bfoley\b|\bfield ?rec", "an ambient texture"),
    ("Music loop", r"\bloops?\b|\bmelod(y|ies)\b|\bmusic\b|\bmelodic\b", "a melodic music loop"),
];

/// Drum one-shot types, used by the kit builder.
pub const DRUMS: [&str; 9] =
    ["Kick", "Snare", "Clap", "Closed hat", "Open hat", "Hi-hat", "Cymbal", "Tom", "Percussion"];

/// Types whose pitch matters, so key and root note are worth showing.
pub const TONAL: [&str; 11] =
    ["808", "Bass", "Pad", "Lead", "Pluck", "Keys", "Chord", "Guitar", "Strings & brass", "Music loop", "Vocal"];

/// Builds a pattern written in this file. They're all tested, so one that doesn't build can't ship; if one ever did,
/// it would match nothing rather than stop the app.
fn re(pattern: &str, ignore_case: bool) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .unwrap_or_else(|_| Regex::new(r"[^\s\S]").unwrap_or_else(|_| unreachable!()))
}

static PATTERNS: LazyLock<Vec<(&'static str, Regex)>> =
    LazyLock::new(|| RULES.iter().map(|(t, p, _)| (*t, re(p, false))).collect());
static CAMEL: LazyLock<Regex> = LazyLock::new(|| re("([a-z])([A-Z])", false));
static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| re(r"[_\-\.\(\)\[\]&+]+", false));
static DIGITS: LazyLock<Regex> = LazyLock::new(|| re(r"([a-z])(\d)", false));
static ONE_SHOT_WORDS: LazyLock<Regex> =
    LazyLock::new(|| re(r"\bone ?shots?\b|\boneshots?\b|\bhits?\b|\bshots?\b", false));
static LOOP_WORDS: LazyLock<Regex> = LazyLock::new(|| re(r"\bloops?\b|\bbpm\b|\b\d{2,3} ?bpm", false));
static BPM_TAG: LazyLock<Regex> = LazyLock::new(|| re(r"(\d{2,3}(?:\.\d+)?)\s*-?bpm", true));
static BARE_NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"(?:^|[_\s-])(\d{2,3})(?:[_\s-]|$)", false));
/// A key in a name: a note, then "maj", "major", "min" or "minor" in any case ("C Minor", "F#MAJ"), or a lone small
/// "m" ("Am"). The C# app only knew the small words, so it read "Pad C Minor" as C major.
static KEY_TAG: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?:^|[_\s\-\(\[])([A-G])([#b♯♭]?)\s?((?i:maj(?:or)?|min(?:or)?)|m)?(?:[_\s\-\)\]]|$)", false)
});

/// Every type, in the order they're tried.
pub fn all() -> impl Iterator<Item = &'static str> {
    RULES.iter().map(|(t, _, _)| *t)
}

/// The text the CLAP model is given for a type ("the sound of a kick drum"), or `None` for a type that isn't one.
pub fn prompt(sound_type: &str) -> Option<String> {
    RULES.iter().find(|(t, _, _)| *t == sound_type).map(|(_, _, p)| format!("the sound of {p}"))
}

/// The type a sample's file name or folders name, or `None`. The file name wins, then folders from the innermost out
/// (four at most).
pub fn from_path(path: &str) -> Option<&'static str> {
    let parts: Vec<&str> = path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let last = parts.len().checked_sub(1)?;
    let first = parts.len().saturating_sub(5);
    (first..=last).rev().find_map(|i| {
        let part = parts.get(i).copied().unwrap_or("");
        let text = normalise(if i == last { stem(part) } else { part });
        PATTERNS.iter().find(|(_, p)| p.is_match(&text)).map(|(t, _)| *t)
    })
}

/// `Some(true)` when the path says it's a loop, `Some(false)` when it says one-shot, else `None`.
pub fn loop_from_path(path: &str) -> Option<bool> {
    let t = normalise(path);
    if ONE_SHOT_WORDS.is_match(&t) {
        Some(false)
    } else if LOOP_WORDS.is_match(&t) {
        Some(true)
    } else {
        None
    }
}

/// A tempo written in the file name ("Loop_128bpm", "128 BPM", "_128_"), if any.
pub fn tempo_from_name(path: &str) -> Option<f64> {
    let name = stem(file_name(path));
    // Only the first of each kind counts, as in the C# app.
    if let Some(bpm) = BPM_TAG.captures(name).and_then(|c| c.get(1)?.as_str().parse::<f64>().ok())
        && (50.0..=220.0).contains(&bpm)
    {
        return Some(bpm);
    }
    BARE_NUMBER
        .captures(name)
        .and_then(|c| c.get(1)?.as_str().parse::<f64>().ok())
        .filter(|bpm| (70.0..=200.0).contains(bpm))
}

/// A key written in the file name ("Am", "F# minor", "_Cmaj_") as (tonic, minor), if any.
pub fn key_from_name(path: &str) -> Option<(i32, bool)> {
    let name = stem(file_name(path));
    let c = KEY_TAG.captures(name)?;
    let part = |i| c.get(i).map_or("", |m| m.as_str());
    music::parse_key(&format!("{}{} {}", part(1), part(2), part(3)))
}

/// Small letters with separators turned into spaces, and camelCase and letters-then-digits split, so "KickHard_01"
/// reads "kick hard 01".
fn normalise(s: &str) -> String {
    let s = CAMEL.replace_all(s, "$1 $2");
    let s = mo_formats::case::to_lower(&s);
    let s = SEPARATORS.replace_all(&s, " ");
    DIGITS.replace_all(&s, "$1 $2").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_builds() {
        for (t, p, _) in RULES {
            assert!(Regex::new(p).is_ok(), "{t}");
        }
        assert_eq!(all().count(), 23);
    }

    #[test]
    fn types_from_names_and_folders() {
        assert_eq!(from_path(r"D:\Samples\Pack\KickHard_01.wav"), Some("Kick"));
        assert_eq!(from_path(r"D:\Samples\Pack\808s\Boom.wav"), Some("808"));
        // The file name wins over the folder.
        assert_eq!(from_path(r"D:\Samples\Kicks\snare_2.wav"), Some("Snare"));
        assert_eq!(from_path("D:/Samples/Open Hats/oh 1.wav"), Some("Open hat"));
        assert_eq!(from_path(r"D:\Samples\Pack\ClosedHat.wav"), Some("Closed hat"));
        assert_eq!(from_path(r"D:\Samples\Pack\Thing.wav"), None);
        // Only four folders up count.
        assert_eq!(from_path(r"D:\Pads\a\b\c\d\Thing.wav"), None);
        assert_eq!(from_path(r"D:\a\Pads\b\c\d\Thing.wav"), Some("Pad"));
        assert_eq!(from_path(""), None);
        // "bd" is a kick once the digits are split off.
        assert_eq!(from_path(r"D:\x\BD01.wav"), Some("Kick"));
    }

    #[test]
    fn loops_tempos_and_keys() {
        assert_eq!(loop_from_path(r"D:\Loops\Kick One Shot.wav"), Some(false));
        assert_eq!(loop_from_path(r"D:\Stuff\Groove_128bpm.wav"), Some(true));
        assert_eq!(loop_from_path(r"D:\Stuff\Thing.wav"), None);

        assert_eq!(tempo_from_name(r"D:\x\Loop_128bpm.wav"), Some(128.0));
        assert_eq!(tempo_from_name(r"D:\x\Loop 92.5 BPM.wav"), Some(92.5));
        assert_eq!(tempo_from_name(r"D:\x\Drums_140_Am.wav"), Some(140.0));
        assert_eq!(tempo_from_name(r"D:\x\Drums_40.wav"), None);
        assert_eq!(tempo_from_name(r"D:\x\Drums_300bpm.wav"), None);

        assert_eq!(key_from_name(r"D:\x\Drums_140_Am.wav"), Some((9, true)));
        assert_eq!(key_from_name(r"D:\x\Pad F# minor.wav"), Some((6, true)));
        assert_eq!(key_from_name(r"D:\x\Stab_Cmaj_.wav"), Some((0, false)));
        assert_eq!(key_from_name(r"D:\x\Stab (Eb).wav"), Some((3, false)));
        assert_eq!(key_from_name(r"D:\x\Bass.wav"), None);
        // Capital words too (the C# app read these as major); a lone capital M isn't minor.
        assert_eq!(key_from_name(r"D:\x\Pad C Minor.wav"), Some((0, true)));
        assert_eq!(key_from_name(r"D:\x\Keys Bb MIN.wav"), Some((10, true)));
        assert_eq!(key_from_name(r"D:\x\Lead F#Major.wav"), Some((6, false)));
        assert_eq!(key_from_name(r"D:\x\Pad G Maj.wav"), Some((7, false)));
    }

    #[test]
    fn prompts() {
        assert_eq!(prompt("Kick").as_deref(), Some("the sound of a kick drum"));
        assert_eq!(prompt("Nope"), None);
    }
}
