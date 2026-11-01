//! What readable text in a file says, for formats (or damaged files) the readers can't decode: audio and plugin paths,
//! and the runs of letters inside binary data. The C# app's `TextScan`.

use crate::plugins::{PLUGIN_EXTENSIONS, ends_with_ignore_case};

/// The file endings audio has.
pub const AUDIO_EXTENSIONS: [&str; 10] =
    [".wav", ".aif", ".aiff", ".flac", ".mp3", ".ogg", ".rx2", ".rex", ".wv", ".m4a"];

/// Whether text names an audio file (zeros and spaces after it aside).
pub fn is_audio_path(s: &str) -> bool {
    let s = s.trim_end_matches(['\0', ' ']);
    AUDIO_EXTENSIONS.iter().any(|e| ends_with_ignore_case(s, e))
}

/// Whether text names a plugin's file.
pub fn is_plugin_path(s: &str) -> bool {
    PLUGIN_EXTENSIONS.iter().any(|e| ends_with_ignore_case(s, e))
}

/// The readable runs of 4 or more characters in binary data: plain ASCII ones first, then UTF-16 (little-endian) ones,
/// each in the order they come.
pub fn strings(data: &[u8]) -> Vec<String> {
    let readable = |b: u8| (0x20..=0x7e).contains(&b);
    let mut out = Vec::new();
    out.extend(
        data.split(|&b| !readable(b)).filter(|run| run.len() >= 4).map(|run| String::from_utf8_lossy(run).into_owned()),
    );

    // A letter then a zero, four or more times, starting anywhere (even an odd byte).
    let mut i = 0;
    while let Some(rest) = data.get(i..).filter(|rest| rest.len() > 1) {
        let pairs = rest.as_chunks::<2>().0;
        let n = pairs.iter().take_while(|&&[b, zero]| readable(b) && zero == 0).count();
        if n >= 4 {
            out.push(pairs.iter().take(n).map(|&[b, _]| b as char).collect());
            i += 2 * n;
        } else {
            i += 1;
        }
    }
    out
}

// Names made safe to save a file under (first written for the Presets page, now shared with the backup).

/// The longest name [`safe_file_name`] gives (UTF-16 units, as .NET counts): longer ones are cut to this.
pub const LONGEST_FILE_NAME: usize = 120;

/// A letter no system takes in a file's name: `< > : " / \ | ? *` and control characters.
pub fn unsafe_in_file_name(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c < ' '
}

/// Names Windows keeps for its devices, whatever ending follows ("CON", "aux.txt", "COM1"), letter case aside.
pub fn is_device_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or_default().trim_end_matches(' ').to_ascii_uppercase();
    let numbered = |prefix: &str| {
        base.strip_prefix(prefix)
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
    };
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT")
}

/// A name every system can save a file under, as the C# `TextScan.SafeFileName`: the letters [`unsafe_in_file_name`]
/// become "_", spaces and dots are trimmed from the ends, it's cut to [`LONGEST_FILE_NAME`], and an empty name is
/// "untitled". Two things the C# app didn't do: a name cut where a space or dot was loses it too (Windows can't open a
/// folder whose name ends in one), and a Windows device's name ("Aux", "CON") gets a "_" in front.
pub fn safe_file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if unsafe_in_file_name(c) { '_' } else { c }).collect();
    let trimmed = cleaned.trim_matches([' ', '.']);
    let named = if is_device_name(trimmed) { format!("_{trimmed}") } else { trimmed.to_owned() };
    let mut out = String::new();
    let mut units = 0;
    for c in named.chars() {
        units += c.len_utf16();
        if units > LONGEST_FILE_NAME {
            break;
        }
        out.push(c);
    }
    let kept = out.trim_end_matches([' ', '.']).len();
    out.truncate(kept);
    if out.is_empty() { "untitled".to_owned() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe_as_the_c_sharp_app_makes_them() {
        assert_eq!(safe_file_name(r#"a<b>c:d"e/f\g|h?i*j"#), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(safe_file_name(" .Bass.. "), "Bass");
        assert_eq!(safe_file_name("tab\there"), "tab_here");
        assert_eq!(safe_file_name("..."), "untitled");
        assert_eq!(safe_file_name(&"x".repeat(200)).len(), 120);
        // Counted as .NET counts: an emoji is two.
        assert_eq!(safe_file_name(&"🎹".repeat(70)).chars().count(), 60);
    }

    #[test]
    fn a_cut_name_never_ends_in_a_space_or_dot_and_a_device_name_is_kept_clear() {
        assert_eq!(safe_file_name(&format!("{} {}", "x".repeat(119), "y".repeat(9))), "x".repeat(119));
        assert_eq!(safe_file_name(&format!("{}.{}", "x".repeat(119), "y".repeat(9))), "x".repeat(119));
        assert_eq!(safe_file_name("Aux"), "_Aux");
        assert_eq!(safe_file_name("con.backup"), "_con.backup");
        assert_eq!(safe_file_name("COM3"), "_COM3");
        assert_eq!(safe_file_name("Auxiliary"), "Auxiliary");
        assert_eq!(safe_file_name("COM10"), "COM10");
    }

    #[test]
    fn finds_ascii_runs_then_utf16_runs() {
        let mut data = b"\x01Kick\x02ab\x03".to_vec();
        data.extend("Snare".encode_utf16().flat_map(|u| u.to_le_bytes()));
        data.extend(b"\x01Hats");
        assert_eq!(strings(&data), ["Kick", "Hats", "Snare"]);
        assert!(is_audio_path("C:\\Kick.WAV\0 "));
        assert!(!is_audio_path("C:\\Kick.wav.txt"));
        assert!(is_plugin_path("Serum_x64.dll"));
    }
}
