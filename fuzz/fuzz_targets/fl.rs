//! FL Studio projects (.flp): everything the app reads from one. A reader may say the file is damaged, but must never
//! panic or hang.
#![no_main]

use libfuzzer_sys::fuzz_target;
use mo_formats::fl::{self, events, presets, song};

fuzz_target!(|data: &[u8]| {
    // What the project list shows.
    let _ = fl::read_bytes(data);
    // The song (channels, patterns, playlist, automation), its notes, its plugin states and its arrangement.
    if let Ok(flp) = events::load(data) {
        let _ = song::read_song_with(&flp, true);
        let _ = song::extract_midi(&flp);
        let _ = presets::extract_presets(&flp);
        let _ = fl::arrangement::read(&flp);
    }
});
