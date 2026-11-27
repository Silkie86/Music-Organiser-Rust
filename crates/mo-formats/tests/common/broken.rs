//! Broken copies of files and a way to tell a reader panicked on one: shared by the broken-file tests of mo-formats,
//! mo-presets and mo-audio (the other two take this file by its path), so a fix here reaches all three.

#![allow(dead_code)]

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
pub fn panic_in(read: impl FnOnce()) -> Option<String> {
    note_panics();
    PANICKED.with(|p| p.borrow_mut().take());
    let caught = mo_catch::catch(std::panic::AssertUnwindSafe(read));
    let said = PANICKED.with(|p| p.borrow_mut().take());
    if caught.is_err() { Some(said.unwrap_or_else(|| "a panic".into())) } else { said }
}

/// A small, fixed random number maker (xorshift), so the broken copies are the same on every run.
pub struct Dice(u64);

impl Dice {
    /// A run of its own for each name: the same name always gives the same run.
    pub fn named(name: &str) -> Dice {
        // FNV-1a of the name; never 0, which xorshift would keep at 0.
        let hash =
            name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        Dice(hash | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// Values that make a length, a count or a place go wrong.
const ODD_BYTES: [u8; 7] = [0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff, 0x40];
const ODD_WORDS: [u32; 8] = [0, 1, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 0xffff, 0x10000, 0x0100_0000];

/// The broken copies of one file: cut short at `cuts` places spread through it (the first is the empty file) and one
/// byte from its end, and `changes` copies with one to four small changes each. With `head`, three changes in four
/// fall in the file's first `head` bytes, where a header's sizes and counts are. Each comes with how it was made.
pub fn broken(
    file: &[u8],
    cuts: usize,
    changes: usize,
    head: Option<usize>,
    dice: &mut Dice,
) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let len = file.len();
    for i in 0..cuts {
        let at = len * i / cuts.max(1);
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
            let at = match head {
                Some(head) if dice.below(4) > 0 => dice.below(f.len().min(head)),
                _ => dice.below(f.len()),
            };
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
