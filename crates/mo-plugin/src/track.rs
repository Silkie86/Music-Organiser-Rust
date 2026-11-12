//! What the track's name says: a plugin on a track called "Kick" opens on kicks, one on "Bass 2" on bass sounds and
//! basslines. Only the first time the list shows; the filters are the user's after that.

/// The `choices` (sound types, or kinds of MIDI part) the track's `name` names, as the choices are written: a word of
/// the name that is one (any capitals, a plural taken as one: "Kicks" is "Kick", "Basslines" is "Bass"), or the
/// whole name run together ("Hi Hat" for "HiHat"). None for a name that names none, or more than two.
pub fn named<'a>(name: &str, choices: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let words: Vec<String> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !w.chars().all(|c| c.is_ascii_digit()) || *w == "808" || *w == "909")
        .map(one)
        .collect();
    if words.is_empty() {
        return Vec::new();
    }
    let joined = words.concat();
    let mut found: Vec<String> = Vec::new();
    for choice in choices {
        let c = one(choice);
        if !c.is_empty() && (words.contains(&c) || joined == c) && !found.iter().any(|f| one(f) == c) {
            found.push(choice.to_owned());
        }
    }
    if found.len() > 2 { Vec::new() } else { found }
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

    const TYPES: [&str; 8] = ["Kick", "Snare", "HiHat", "Bass", "Pad", "808", "FX", "Vocal"];

    #[test]
    fn a_track_name_picks_the_types_it_names() {
        assert_eq!(named("Kick", TYPES), ["Kick"]);
        assert_eq!(named("KICKS 2", TYPES), ["Kick"]);
        assert_eq!(named("Hi Hat", TYPES), ["HiHat"]);
        assert_eq!(named("Bass synth", TYPES), ["Bass"]);
        assert_eq!(named("808 bass", TYPES), ["Bass", "808"]);
        assert_eq!(named("Pads & vocals", TYPES), ["Pad", "Vocal"]);
        assert!(named("Audio 3", TYPES).is_empty(), "a name that says nothing picks nothing");
        assert!(named("", TYPES).is_empty());
        assert!(named("Kick snare pad", TYPES).is_empty(), "too many: it's a mix, not one kind");
    }

    #[test]
    fn a_track_name_picks_the_kind_of_midi_part() {
        let kinds = ["Chords", "Melodies", "Basslines", "Arps", "Drums", "Other"];
        assert_eq!(named("Bass", kinds), ["Basslines"]);
        assert_eq!(named("Lead melody", kinds), ["Melodies"]);
        assert_eq!(named("Chord stabs", kinds), ["Chords"]);
        assert_eq!(named("Drums", kinds), ["Drums"]);
        assert!(named("Kick", kinds).is_empty());
    }
}
