//! Bitwig projects (.bwproject), the first step only: decoding the file's objects. It may say the file isn't Bitwig's
//! or is damaged, but must never panic or hang.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mo_formats::bitwig::decode(data);
});
