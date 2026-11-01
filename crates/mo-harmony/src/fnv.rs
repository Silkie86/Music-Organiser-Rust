//! FNV-1a over bytes, 64 bits: a hash that's the same on every machine and in every version of Rust, for the checks
//! that keep the answers the app saves honest (rule 7): the key finder's and the chord reader's numbers and readings.

#[derive(Clone, Copy, Debug)]
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) const fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub(crate) fn number(&mut self, n: i128) {
        self.bytes(&n.to_le_bytes());
    }

    pub(crate) fn real(&mut self, x: f64) {
        self.bytes(&x.to_bits().to_le_bytes());
    }

    pub(crate) const fn finish(self) -> u64 {
        self.0
    }
}
