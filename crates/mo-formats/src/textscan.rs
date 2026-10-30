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

#[cfg(test)]
mod tests {
    use super::*;

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
