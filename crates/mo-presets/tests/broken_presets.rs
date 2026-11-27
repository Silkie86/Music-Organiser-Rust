//! The preset readers (.fxp, .fxb, .vstpreset, .bwpreset, and Serum 1's settings in them) on broken copies of
//! made-up presets and the recipe's Bitwig preset: cut short at many places, and changed in a few bytes. A reader may
//! say the file isn't a preset, but must never panic or take minutes. The fuzzer (`fuzz/README.md`, target
//! `presets`) does this far more deeply; this runs on every change, with the same broken copies each time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::time::{Duration, Instant};

use mo_formats::presets::{fxp, vstpreset};

#[path = "../../mo-formats/tests/common/broken.rs"]
mod broken;

use broken::*;

#[test]
fn preset_readers_survive_broken_files() {
    let chunk = fxp::wrap_chunk(b"a plugin's own settings, long enough to have a middle", "Broken", 0x5866_6572);
    let params = fxp::wrap_params(&[0.0, 0.25, 0.5, 1.0], "Broken", 0x5866_6572);
    let bank = mo_formats::presets::fxp_to_bank(&chunk, 0);
    let vst3 = vstpreset::build("56535458565364736572756d00000000", b"component state", b"controller state");
    let recipe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mo-formats/tests/fixtures/recipe/bitwig");
    let bwpreset = std::fs::read(recipe.join("MO Recipe Polysynth.bwpreset")).unwrap();
    let files = [
        ("Broken.fxp", chunk),
        ("Broken.fxp", params),
        ("Broken.fxb", bank),
        ("Broken.vstpreset", vst3),
        ("Broken.bwpreset", bwpreset),
    ];
    let mut dice = Dice::named("presets");
    for (n, (name, file)) in files.iter().enumerate() {
        for (how, bytes) in broken(file, 24, 150, Some(256), &mut dice) {
            let started = Instant::now();
            let said = panic_in(|| {
                let _ = mo_presets::files::read_bytes(&bytes, name);
                let _ = mo_presets::bitwig::inspect_bytes(&bytes);
                let ext = name.rsplit('.').next().unwrap();
                let _ = mo_presets::serum2::serum_1_chunk(&bytes, ext);
            });
            if let Some(said) = said {
                panic!("{name} file {n}, {how}: the reader panicked: {said}");
            }
            let took = started.elapsed();
            assert!(took < Duration::from_secs(20), "{name} file {n}, {how}: it took {took:?}");
        }
    }
}
