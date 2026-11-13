//! What the track's name says: a plugin on a track called "Kick" opens on kicks, one on "CH" on closed hats, one on
//! "Bass 2" on bass sounds and basslines. Only the first time the list shows; the filters are the user's after that.
//!
//! A name is read with the app's own clues for a sound's type (`mo_sounds::types`: "BD" and "Kik" are kicks, "CHH"
//! and "C Hat" closed hats, "OH" open hats, "HH" hi-hats, "Vox" vocals, "Sub" 808s), the few short names producers give
//! tracks that a sample's name rarely has ("Cym", "Pno", "Ld"), and the choices' own words.

use mo_sounds::types;

/// Short names for tracks that the type clues don't know: each, as a word of the name, reads as the words after it.
const SHORT: [(&str, &str); 16] = [
    ("cym", "cymbal"),
    ("cyms", "cymbals"),
    ("cch", "closed hat"),
    ("hhc", "closed hat"),
    ("hho", "open hat"),
    ("kd", "kick"),
    ("sn", "snare"),
    ("shkr", "shaker"),
    ("tamb", "tambourine"),
    ("voc", "vocal"),
    ("vocs", "vocals"),
    ("vx", "vox"),
    ("ld", "lead"),
    ("pno", "piano"),
    ("syn", "synth"),
    ("arpeggio", "arp"),
];

/// The hi-hats: a track for hats in general ("Hats", "HH") shows all three; they count as one kind of sound.
const HATS: [&str; 3] = ["Hi-hat", "Closed hat", "Open hat"];

/// The kind of MIDI part a type of sound is played as (the MIDI tab's kinds, as its chips name them).
fn part_kind(sound_type: &str) -> Option<&'static str> {
    match sound_type {
        "808" | "Bass" => Some("Basslines"),
        "Lead" | "Pluck" | "Vocal" | "Music loop" => Some("Melodies"),
        "Pad" | "Keys" | "Chord" | "Strings & brass" | "Guitar" => Some("Chords"),
        t if types::DRUMS.contains(&t) || t == "Drum loop" => Some("Drums"),
        _ => None,
    }
}

/// The `choices` (sound types, or kinds of MIDI part) the track's `name` names, as the choices are written. None for
/// a name that names none, or more than two kinds of sound (a mix bus): "Kick & Snare" picks both, "Drums" (every drum
/// type) picks the Drums kind of part.
pub fn named<'a>(name: &str, choices: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let choices: Vec<&str> = choices.into_iter().collect();
    let name = lengthened(name);
    // Each found as (the choice, the kind of sound it counts as).
    let mut found: Vec<(String, String)> = Vec::new();
    let mut typed = types::all_named(&name);
    // The clues for a sample's whole name, narrowed for a track's: a kind of hat named isn't hats in general ("C Hat"
    // also says "hat"); a loop of one kind isn't a music loop ("Perc loop"); a bass drum isn't a bass.
    if typed.iter().any(|t| *t == "Closed hat" || *t == "Open hat") {
        typed.retain(|t| *t != "Hi-hat");
    }
    if typed.len() > 1 {
        typed.retain(|t| *t != "Music loop");
    }
    let lower = name.to_lowercase();
    if typed.contains(&"Kick") && (lower.contains("bass drum") || lower.contains("bassdrum")) {
        typed.retain(|t| *t != "Bass");
    }
    let only_hats = typed.contains(&"Hi-hat") && !typed.iter().any(|t| *t == "Closed hat" || *t == "Open hat");
    for t in &typed {
        let hat = HATS.contains(t);
        let kind = if hat { "hats" } else { t };
        if *t == "Hi-hat" && only_hats {
            for h in HATS {
                take(&mut found, &choices, h, kind);
            }
        } else {
            take(&mut found, &choices, t, kind);
        }
        // Only a kind of part as the MIDI tab names it ("Basslines" is never the type Bass).
        if let Some(part) = part_kind(t)
            && let Some(choice) = choices.iter().find(|c| c.eq_ignore_ascii_case(part))
        {
            take(&mut found, &choices, choice, part);
        }
    }
    // A choice the clues don't know (a type of the library's own, a kind of part) by its own word.
    if found.is_empty() {
        for choice in words_named(&name, &choices) {
            take(&mut found, &choices, &choice, &choice);
        }
    }
    let mut kinds: Vec<&str> = found.iter().map(|(_, k)| k.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    if kinds.len() > 2 { Vec::new() } else { found.into_iter().map(|(c, _)| c).collect() }
}

/// Keeps the choice that is `wanted` (as [`same`] matches), counted as `kind` of sound, once.
fn take(found: &mut Vec<(String, String)>, choices: &[&str], wanted: &str, kind: &str) {
    if let Some(choice) = choices.iter().find(|c| same(c, wanted))
        && !found.iter().any(|(f, _)| f == choice)
    {
        found.push(((*choice).to_owned(), kind.to_owned()));
    }
}

/// The name with its short words written out ("Cym 2" is "cymbal 2").
fn lengthened(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| SHORT.iter().find(|(s, _)| w.eq_ignore_ascii_case(s)).map_or(w, |(_, long)| long))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a choice is the type or kind wanted: in any case, a plural taken as one.
fn same(choice: &str, wanted: &str) -> bool {
    choice.eq_ignore_ascii_case(wanted) || one(choice) == one(wanted)
}

/// The `choices` a word of `name` is (any capitals, a plural taken as one: "Kicks" is "Kick", "Basslines" is "Bass"),
/// or the whole name run together is ("Hi Hat" for "HiHat").
fn words_named(name: &str, choices: &[&str]) -> Vec<String> {
    let words: Vec<String> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()) || *w == "808" || *w == "909")
        .map(one)
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let joined = words.concat();
    choices
        .iter()
        .filter(|choice| {
            let c = one(choice);
            !c.is_empty() && (words.contains(&c) || joined == c)
        })
        .map(|c| (*c).to_owned())
        .collect()
}

/// A word as it's matched: in lower case, one of it ("kicks" is "kick", "basslines" is "bass", "pads" is "pad").
fn one(word: &str) -> String {
    let w = word.trim().to_lowercase();
    for (many, single) in [("basslines", "bass"), ("bassline", "bass"), ("melodies", "melody"), ("hats", "hat")] {
        if w == many {
            return single.to_owned();
        }
    }
    match w.strip_suffix('s') {
        // "bass", "glass" and "fx"-like words keep their s.
        Some(stem) if stem.len() > 2 && !w.ends_with("ss") => stem.to_owned(),
        _ => w,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's own types, as a library has them.
    fn library() -> Vec<&'static str> {
        types::all().collect()
    }

    #[test]
    fn a_track_name_picks_the_types_it_names() {
        let lib = library();
        let named = |name: &str| named(name, lib.iter().copied());
        assert_eq!(named("Kick"), ["Kick"]);
        assert_eq!(named("KICKS 2"), ["Kick"]);
        assert_eq!(named("Bass synth"), ["Bass"]);
        assert_eq!(named("808 bass"), ["808", "Bass"]);
        assert_eq!(named("Pads & vocals"), ["Vocal", "Pad"]);
        assert!(named("Audio 3").is_empty(), "a name that says nothing picks nothing");
        assert!(named("").is_empty());
        assert!(named("Kick snare pad").is_empty(), "too many: it's a mix, not one kind");
    }

    #[test]
    fn the_short_names_producers_give_tracks() {
        let lib = library();
        let named = |name: &str| named(name, lib.iter().copied());
        for kick in ["BD", "Kik", "KD 1", "Bass drum", "kck"] {
            assert_eq!(named(kick), ["Kick"], "{kick}");
        }
        for snare in ["SD", "Snr", "SN", "Rim"] {
            assert_eq!(named(snare), ["Snare"], "{snare}");
        }
        for closed in ["CH", "CHH", "C Hat", "Closed hat", "Closed HiHat", "CCH", "HHC"] {
            assert_eq!(named(closed), ["Closed hat"], "{closed}");
        }
        for open in ["OH", "OHH", "O Hat", "Open hat", "HHO"] {
            assert_eq!(named(open), ["Open hat"], "{open}");
        }
        // Hats in general: all three, as one kind of sound.
        for hats in ["HH", "Hats", "Hi-Hat", "HiHats", "hi hat 2"] {
            assert_eq!(named(hats), ["Hi-hat", "Closed hat", "Open hat"], "{hats}");
        }
        assert_eq!(named("CH + OH"), ["Closed hat", "Open hat"]);
        assert_eq!(named("Cym"), ["Cymbal"]);
        assert_eq!(named("Crash"), ["Cymbal"]);
        assert_eq!(named("Perc loop"), ["Percussion"]);
        assert_eq!(named("Shkr"), ["Percussion"]);
        assert_eq!(named("Vox"), ["Vocal"]);
        assert_eq!(named("Ld"), ["Lead"]);
        assert_eq!(named("Sub"), ["808"]);
        assert_eq!(named("Pno"), ["Keys"]);
        assert_eq!(named("Gtr"), ["Guitar"]);
        assert_eq!(named("SFX"), ["FX"]);
    }

    #[test]
    fn only_the_types_the_library_has() {
        assert_eq!(named("Kick", ["Snare", "Kick"]), ["Kick"]);
        assert!(named("Kick", ["Snare"]).is_empty());
        // A library with only one kind of hat: that one.
        assert_eq!(named("Hats", ["Kick", "Open hat"]), ["Open hat"]);
        // A type of its own that the clues don't know, named by its word.
        assert_eq!(named("Glitch", ["Glitch", "Kick"]), ["Glitch"]);
    }

    #[test]
    fn a_track_name_picks_the_kind_of_midi_part() {
        let kinds = ["Chords", "Melodies", "Basslines", "Arps", "Drums", "Other"];
        assert_eq!(named("Bass", kinds), ["Basslines"]);
        assert_eq!(named("Sub", kinds), ["Basslines"]);
        assert_eq!(named("Lead melody", kinds), ["Melodies"]);
        assert_eq!(named("Ld", kinds), ["Melodies"]);
        assert_eq!(named("Chord stabs", kinds), ["Chords"]);
        assert_eq!(named("Keys", kinds), ["Chords"]);
        assert_eq!(named("Arp", kinds), ["Arps"]);
        assert_eq!(named("Arpeggio", kinds), ["Arps"]);
        assert_eq!(named("Drums", kinds), ["Drums"]);
        assert_eq!(named("Kick", kinds), ["Drums"]);
        assert_eq!(named("HH", kinds), ["Drums"]);
        assert!(named("Audio 3", kinds).is_empty());
    }
}
