//! Note and key names and Camelot codes, written as the C# Sound Lab writes them (`Sounds/MusicTheory.cs`).
//!
//! Only the naming the sounds list needs lives here; finding keys and chords belongs to the harmony engine.

/// Note names with sharps, C first.
pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

// How keys are written: flat keys with flats (Eb major, Bb minor), as charts and pack names write them.
const MAJOR_KEY_NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
const MINOR_KEY_NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];

fn pitch_class(n: i32) -> usize {
    n.rem_euclid(12) as usize
}

/// A key's name: "Eb major", or with `brief` "Eb"; "C# minor", or "C#m".
pub fn key_name(tonic: i32, minor: bool, brief: bool) -> String {
    let names = if minor { &MINOR_KEY_NAMES } else { &MAJOR_KEY_NAMES };
    let note = names.get(pitch_class(tonic)).copied().unwrap_or("C");
    match (brief, minor) {
        (true, true) => format!("{note}m"),
        (true, false) => note.to_owned(),
        (false, true) => format!("{note} minor"),
        (false, false) => format!("{note} major"),
    }
}

/// A MIDI note's name with its octave, middle C (60) being "C4".
pub fn note_name(midi: i32) -> String {
    let note = NOTE_NAMES.get(pitch_class(midi)).copied().unwrap_or("C");
    // Division that cuts toward zero, as C#'s does.
    format!("{note}{}", midi / 12 - 1)
}

/// Reads "C", "F#m", "Bb minor", "A min" as (tonic, minor). `None` when it isn't a key.
pub fn parse_key(text: &str) -> Option<(i32, bool)> {
    let t = text.trim();
    let mut chars = t.chars();
    let first = chars.next()?.to_ascii_uppercase();
    let mut idx = NOTE_NAMES.iter().position(|n| n.len() == 1 && n.starts_with(first))? as i32;
    let mut rest = chars.as_str();
    if let Some(r) = rest.strip_prefix(['#', '♯']) {
        idx += 1;
        rest = r;
    } else if let Some(r) = rest.strip_prefix(['b', '♭']) {
        idx -= 1;
        rest = r;
    }
    let rest = mo_formats::case::to_lower(rest.trim());
    let minor = rest.starts_with('m') && !rest.starts_with("maj");
    Some((idx.rem_euclid(12), minor))
}

/// The Camelot wheel code, for harmonic mixing: A minor is "8A", C major "8B".
pub fn camelot(tonic: i32, minor: bool) -> String {
    // Each major key's place on the wheel, with C at 8.
    let place = |t: i32| match (8 + 7 * t.rem_euclid(12)) % 12 {
        0 => 12,
        n => n,
    };
    if minor { format!("{}A", place(tonic + 3)) } else { format!("{}B", place(tonic)) }
}

/// Keys that mix well with a key: itself, its relative, and its neighbours on the Camelot wheel.
pub fn compatible_keys(tonic: i32, minor: bool) -> [(i32, bool); 4] {
    let t = tonic.rem_euclid(12);
    let relative = if minor { (t + 3) % 12 } else { (t + 9) % 12 };
    [(t, minor), (relative, !minor), ((t + 7) % 12, minor), ((t + 5) % 12, minor)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(key_name(3, false, false), "Eb major");
        assert_eq!(key_name(1, true, true), "C#m");
        assert_eq!(key_name(-2, false, true), "Bb");
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(36), "C2");
        assert_eq!(note_name(0), "C-1");
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_key("F#m"), Some((6, true)));
        assert_eq!(parse_key("Bb minor"), Some((10, true)));
        assert_eq!(parse_key("cb"), Some((11, false)));
        assert_eq!(parse_key("A maj"), Some((9, false)));
        assert_eq!(parse_key("H"), None);
        assert_eq!(parse_key("  "), None);
    }

    #[test]
    fn camelot_codes() {
        assert_eq!(camelot(9, true), "8A");
        assert_eq!(camelot(0, false), "8B");
        assert_eq!(camelot(7, false), "9B");
        assert_eq!(camelot(5, false), "7B");
        assert_eq!(camelot(4, true), "9A");
        assert_eq!(camelot(6, false), "2B");
    }
}
