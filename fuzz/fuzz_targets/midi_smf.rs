//! Standard MIDI Files (.mid). The reader may say the file is damaged, but must never panic or hang.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mo_formats::midi::smf::read_from(data, Path::new("fuzz.mid"));
});
