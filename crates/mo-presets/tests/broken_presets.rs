//! The preset readers (.fxp, .fxb, .vstpreset, .bwpreset, and Serum 1's settings in them) on broken copies of
//! made-up presets and the recipe's Bitwig preset: cut short at many places, and changed in a few bytes. A reader may
//! say the file isn't a preset, but must never panic or take minutes. The fuzzer (`fuzz/README.md`, target
//! `presets`) does this far more deeply; this runs on every change, with the same broken copies each time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::time::{Duration, Instant};

use mo_formats::presets::{fxp, vstpreset};

thread_local! {
    /// What the last panic on this thread said, even one a reader caught itself (that is still a reader stopping on
    /// a file, and its file loses what it would have shown).
    static PANICKED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Notes every panic on the thread it happens on (once per test program), as well as printing it as usual.
fn note_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let usual = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let said = format!("{} at {:?}", info.payload_as_str().unwrap_or("a panic"), info.location());
            let _ = PANICKED.try_with(|p| *p.borrow_mut() = Some(said));
            usual(info);
        }));
    });
}

/// Runs `read`, and says what made it panic, if anything did (caught by the reader itself or not).
fn panic_in(read: impl FnOnce()) -> Option<String> {
    note_panics();
    PANICKED.with(|p| p.borrow_mut().take());
    let caught = mo_catch::catch(std::panic::AssertUnwindSafe(read));
    let said = PANICKED.with(|p| p.borrow_mut().take());
    if caught.is_err() { Some(said.unwrap_or_else(|| "a panic".into())) } else { said }
}

/// A small, fixed random number maker (xorshift), so the broken copies are the same on every run.
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// Values that make a length, a count or a place go wrong.
const ODD_BYTES: [u8; 7] = [0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff, 0x40];
const ODD_WORDS: [u32; 8] = [0, 1, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 0xffff, 0x10000, 0x0100_0000];

/// The broken copies of one file: cut short at `cuts` places spread through it, and `changes` copies with one to four
/// small changes each. Each comes with how it was made.
fn broken(file: &[u8], cuts: usize, changes: usize, dice: &mut Dice) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let len = file.len();
    for i in 0..=cuts {
        let at = len * i / (cuts + 1);
        out.push((format!("cut at {at}"), file[..at].to_vec()));
    }
    if len > 1 {
        out.push((format!("cut at {}", len - 1), file[..len - 1].to_vec()));
    }
    if len == 0 {
        return out;
    }
    for _ in 0..changes {
        let mut f = file.to_vec();
        let mut how = Vec::new();
        for _ in 0..1 + dice.below(4) {
            let at = dice.below(f.len());
            match dice.below(6) {
                0 => {
                    let bit = dice.below(8);
                    f[at] ^= 1 << bit;
                    how.push(format!("bit {bit} of byte {at} flipped"));
                }
                1 => {
                    let b = ODD_BYTES[dice.below(ODD_BYTES.len())];
                    f[at] = b;
                    how.push(format!("byte {at} set to {b:#04x}"));
                }
                2 if at + 4 <= f.len() => {
                    let w = ODD_WORDS[dice.below(ODD_WORDS.len())];
                    let bytes = if dice.below(2) == 0 { w.to_le_bytes() } else { w.to_be_bytes() };
                    f[at..at + 4].copy_from_slice(&bytes);
                    how.push(format!("bytes {at}..{} set to {bytes:02x?}", at + 4));
                }
                3 => {
                    let n = 1 + dice.below(64.min(f.len() - at));
                    f.drain(at..at + n);
                    how.push(format!("{n} bytes taken out at {at}"));
                }
                4 => {
                    let n = 1 + dice.below(64.min(f.len() - at));
                    let part = f[at..at + n].to_vec();
                    f.splice(at..at, part);
                    how.push(format!("{n} bytes at {at} doubled"));
                }
                _ => {
                    let from = dice.below(f.len());
                    let n = 1 + dice.below(16.min(f.len() - from).min(f.len() - at));
                    let part = f[from..from + n].to_vec();
                    f[at..at + n].copy_from_slice(&part);
                    how.push(format!("{n} bytes from {from} copied over {at}"));
                }
            }
            if f.is_empty() {
                break;
            }
        }
        out.push((how.join(", "), f));
    }
    out
}

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
    let mut dice = Dice(0x1234_5678_9abc_def1);
    for (n, (name, file)) in files.iter().enumerate() {
        for (how, bytes) in broken(file, 24, 150, &mut dice) {
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
