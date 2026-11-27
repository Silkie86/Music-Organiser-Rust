//! Plugin preset files (.fxp, .fxb, .vstpreset, .bwpreset): what the Presets page reads from each, and the Serum 1
//! settings it takes out of one. The first byte picks the file's ending, the rest is the file. A reader may say the
//! file isn't a preset, but must never panic or hang.
#![no_main]

use libfuzzer_sys::fuzz_target;

const ENDINGS: &[&str] = &["Fuzz.fxp", "Fuzz.fxb", "Fuzz.vstpreset", "Fuzz.bwpreset"];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, raw)) = data.split_first() else { return };
    let name = ENDINGS[usize::from(pick) % ENDINGS.len()];
    let _ = mo_presets::files::read_bytes(raw, name);
    // A Bitwig preset looked at as a template for building presets.
    let _ = mo_presets::bitwig::inspect_bytes(raw);
    // Serum 1's settings out of a preset or a song's plugin state.
    let ext = name.rsplit('.').next().unwrap_or_default();
    let _ = mo_presets::serum2::serum_1_chunk(raw, ext);
});
