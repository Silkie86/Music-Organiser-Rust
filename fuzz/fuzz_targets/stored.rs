//! What the app keeps in its own databases, read back: a MIDI part's notes and features, its chords and members, and a
//! sound's held notes. A database can be damaged too (a full disk or a crash while it was written), and the CLAP
//! plugin reads these inside the DAW, where a panic would take the DAW down. The first byte picks the reader. Each may
//! give nothing for damaged bytes, but must never panic or hang.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&pick, raw)) = data.split_first() else { return };
    match pick % 4 {
        0 => {
            let _ = mo_midi::store::decode_notes(raw);
            let _ = mo_midi::index::decode_notes(raw);
        }
        1 => {
            let _ = mo_midi::index::features_of(raw);
            let _ = mo_midi::catalogue::likeness(raw, raw.get(raw.len() / 2..).unwrap_or_default());
        }
        2 => {
            let text = String::from_utf8_lossy(raw);
            let _ = mo_midi::chord::PhraseChord::decode(&text);
            let _ = mo_midi::member::PhraseMember::decode(&text);
        }
        _ => {
            let _ = mo_sounds::index::held_from_bytes(raw);
        }
    }
});
