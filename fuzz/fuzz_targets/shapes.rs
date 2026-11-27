//! Curve and shape files (Serum, LFO Tool, Vital, Duck, Bitwig curves) and the knob moves found in a song: everything the
//! Curves page reads. The first byte picks the file's ending, the rest is the file. A reader may say the file is
//! damaged, but must never panic or hang.
#![no_main]

use libfuzzer_sys::fuzz_target;

/// The endings the Curves page reads, each by its own reader.
const ENDINGS: &[&str] = &[
    "Fuzz.XferShape",
    "Fuzz.shp",
    "Fuzz.fxp",
    "Fuzz.vitallfo",
    "Fuzz.vital",
    "Fuzz.dmlfo",
    "DeviousMachines/Duck/Fuzz.preset",
    "Fuzz.bwcurve",
    "Fuzz.flp",
    "Fuzz.bwproject",
];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, raw)) = data.split_first() else { return };
    let name = ENDINGS[usize::from(pick) % ENDINGS.len()];
    let path = format!("/mo-fuzz-nowhere/{name}");
    // What the Curves page scan does with the file (it picks the reader by the ending).
    let _ = mo_presets::curves::found_in(&path, raw.to_vec(), 0);
    // And each shape reader on its own, as the app's other places call them.
    let _ = mo_formats::serum::read(&path, raw);
    let _ = mo_formats::vital::read(&path, raw);
    let _ = mo_formats::duck::read(&path, raw);
    let _ = mo_formats::bitwig::curve::read(raw.to_vec());
});
