//! MIDI Clip Files (.midi2, MIDI 2.0). The reader may say the file is damaged, but must never panic or hang.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mo_formats::midi::clip::read_bytes(data);
});
