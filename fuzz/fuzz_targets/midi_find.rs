//! The MIDI page's looks into a file: the parts the index finds in a MIDI file or in any song (FL Studio, Bitwig,
//! Ableton, Studio One), and the song as it plays. The first byte picks the file's ending, the rest is the file. A reader may say the file is damaged, but
//! must never panic or hang.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

const ENDINGS: &[&str] = &["fuzz.mid", "fuzz.midi2", "fuzz.flp", "fuzz.bwproject", "fuzz.als", "fuzz.song"];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, raw)) = data.split_first() else { return };
    let path = Path::new("/mo-fuzz-nowhere").join(ENDINGS[usize::from(pick) % ENDINGS.len()]);
    let _ = mo_midi::extract::from_bytes(&path, raw, &[]);
    let _ = mo_midi::playing::from_bytes(&path, raw);
    let _ = mo_formats::midi::read_from(raw, &path);
});
