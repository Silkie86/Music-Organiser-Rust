//! Bitwig projects (.bwproject): everything the app reads on top of the decoded file. A reader may say the file is
//! damaged, but must never panic or hang.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use mo_formats::bitwig::{self, Project};

fuzz_target!(|data: &[u8]| {
    // A folder that doesn't exist, so the reader's look for audio beside the project finds nothing on this PC.
    let path = Path::new("/mo-fuzz-nowhere/fuzz.bwproject");
    // What the project list shows (this also scans the text of a file that won't decode).
    let _ = bitwig::read_from(data, path);
    if let Ok(project) = Project::open(data.to_vec()) {
        let _ = project.midi();
        let _ = project.presets();
        let _ = bitwig::arrangement::read(&project.doc);
        let _ = bitwig::automation::clips(&project.doc);
    }
});
