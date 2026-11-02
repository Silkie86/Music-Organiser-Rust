//! Studio One songs (.song, a zip of XML files): everything the app reads from one. A reader may say the file is
//! damaged, but must never panic or hang.
#![no_main]

use libfuzzer_sys::fuzz_target;
use mo_formats::studio_one::Song;

fuzz_target!(|data: &[u8]| {
    if let Ok(mut song) = Song::open(data) {
        let _ = song.info();
        let _ = song.midi();
        let _ = song.presets();
        let _ = song.arrangement();
    }
});
