//! Ableton Live sets (.als, gzipped XML): everything the app reads from one. A reader may say the file is damaged, but
//! must never panic or hang.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use mo_formats::ableton::LiveSet;

fuzz_target!(|data: &[u8]| {
    // A folder that doesn't exist, so the reader's look for audio beside the set finds nothing on this PC.
    let folder = Path::new("/mo-fuzz-nowhere");
    if let Ok(set) = LiveSet::open(data) {
        let set = set.in_folder(Some(folder));
        // What the project list shows, its notes and its arrangement.
        let _ = set.info();
        let _ = set.midi();
        let _ = set.arrangements();
    }
});
