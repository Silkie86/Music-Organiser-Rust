//! A song's shape (its length, parts and how much moves) for the Board, and its arrangement, from an FL Studio, Bitwig,
//! Ableton or Studio One save. The first byte picks the file's ending, the rest is the file. A reader may say the file
//! is damaged, but must never panic or hang.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

const ENDINGS: &[&str] = &["fuzz.flp", "fuzz.bwproject", "fuzz.als", "fuzz.song"];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, raw)) = data.split_first() else { return };
    let path = Path::new("/mo-fuzz-nowhere").join(ENDINGS[usize::from(pick) % ENDINGS.len()]);
    let _ = mo_formats::shape::read_bytes(&path, raw);
    let _ = mo_formats::arrangement::read_bytes(&path, raw);
});
