//! The twelve notes of the octave, with the octave left aside (pitch classes), and sets of them.
//!
//! Harmony mostly doesn't care which octave a note is in: C2 under a chord and C5 on top of it are both the chord's C.
//! So keys and chords are worked out on pitch classes, and a set of them fits in twelve bits.

use std::fmt;

/// One of the twelve notes with the octave left aside: 0 is C, 1 is C♯ or D♭, and so on up to 11, B.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PitchClass(u8);

impl PitchClass {
    pub const C: PitchClass = PitchClass(0);
    pub const D: PitchClass = PitchClass(2);
    pub const E: PitchClass = PitchClass(4);
    pub const F: PitchClass = PitchClass(5);
    pub const G: PitchClass = PitchClass(7);
    pub const A: PitchClass = PitchClass(9);
    pub const B: PitchClass = PitchClass(11);

    /// All twelve, from C up.
    // The loop stays inside the array.
    #[allow(clippy::indexing_slicing)]
    pub const ALL: [PitchClass; 12] = {
        let mut all = [PitchClass(0); 12];
        let mut i = 0;
        while i < 12 {
            all[i] = PitchClass(i as u8);
            i += 1;
        }
        all
    };

    /// The pitch class any whole number of semitones above C lands on, below C too: 12 is C again, -1 is B.
    pub const fn new(semitones: i32) -> PitchClass {
        PitchClass(semitones.rem_euclid(12) as u8)
    }

    /// The pitch class of a MIDI key (60 is a C in every DAW, whatever octave number it shows).
    pub const fn of_key(key: u8) -> PitchClass {
        PitchClass(key % 12)
    }

    /// 0 for C up to 11 for B.
    pub const fn number(self) -> u8 {
        self.0
    }

    /// 0 for C up to 11 for B, to pick from a table of twelve.
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// This many semitones up (down when negative), round the octave.
    pub const fn up(self, semitones: i32) -> PitchClass {
        PitchClass::new(self.0 as i32 + semitones)
    }

    /// How many semitones up from `from` this is, 0 to 11: E is 4 above C, and C is 8 above E.
    pub const fn above(self, from: PitchClass) -> u8 {
        (self.0 + 12 - from.0) % 12
    }
}

impl fmt::Display for PitchClass {
    /// The name with a flat for the black notes but F♯, as jazz charts write a chord's root: C, Db, D … F#, G, Ab.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(crate::names::name(&crate::names::FLATS_BUT_F_SHARP, *self))
    }
}

/// A set of pitch classes in twelve bits, bit 0 for C up to bit 11 for B. Also used for a chord's intervals above its
/// root (bit 0 the root, bit 4 a major third, bit 10 a flat seventh).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PcSet(u16);

impl PcSet {
    pub const EMPTY: PcSet = PcSet(0);
    pub const ALL: PcSet = PcSet(0xFFF);

    /// The set these twelve bits hold (any higher bits are dropped).
    pub const fn from_bits(bits: u16) -> PcSet {
        PcSet(bits & 0xFFF)
    }

    /// The set of these pitch classes, or of these intervals above a root (each taken round the octave).
    // The loop stays inside the list (a const fn can't use an iterator).
    #[allow(clippy::indexing_slicing)]
    pub const fn of(numbers: &[i32]) -> PcSet {
        let mut bits = 0u16;
        let mut i = 0;
        while i < numbers.len() {
            bits |= 1 << PitchClass::new(numbers[i]).0;
            i += 1;
        }
        PcSet(bits)
    }

    /// The twelve bits, bit 0 for C.
    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, pc: PitchClass) -> bool {
        self.0 >> pc.0 & 1 != 0
    }

    /// Whether the interval this many semitones above the root is in a set of intervals (any whole number, round
    /// the octave: 14 is the 9th, the same as 2).
    pub const fn has(self, interval: i32) -> bool {
        self.contains(PitchClass::new(interval))
    }

    pub const fn with(self, pc: PitchClass) -> PcSet {
        PcSet(self.0 | 1 << pc.0)
    }

    pub const fn without(self, pc: PitchClass) -> PcSet {
        PcSet(self.0 & !(1 << pc.0))
    }

    pub const fn union(self, other: PcSet) -> PcSet {
        PcSet(self.0 | other.0)
    }

    pub const fn intersection(self, other: PcSet) -> PcSet {
        PcSet(self.0 & other.0)
    }

    pub const fn difference(self, other: PcSet) -> PcSet {
        PcSet(self.0 & !other.0)
    }

    /// How many pitch classes are in it.
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every pitch class moved up this many semitones (down when negative).
    pub const fn transposed(self, semitones: i32) -> PcSet {
        let s = PitchClass::new(semitones).0 as u32;
        if s == 0 {
            return self;
        }
        PcSet(((self.0 << s) | (self.0 >> (12 - s))) & 0xFFF)
    }

    /// The set as intervals above `root`: the root becomes bit 0. The opposite of [`PcSet::on`].
    pub const fn above(self, root: PitchClass) -> PcSet {
        self.transposed(-(root.0 as i32))
    }

    /// A set of intervals placed on `root`: the pitch classes they land on. The opposite of [`PcSet::above`].
    pub const fn on(self, root: PitchClass) -> PcSet {
        self.transposed(root.0 as i32)
    }

    /// The lowest pitch class in it from C up, if any.
    pub const fn first(self) -> Option<PitchClass> {
        if self.0 == 0 { None } else { Some(PitchClass(self.0.trailing_zeros() as u8)) }
    }

    /// The pitch classes in it, from C up.
    pub fn iter(self) -> impl Iterator<Item = PitchClass> {
        let mut bits = self.0;
        std::iter::from_fn(move || {
            if bits == 0 {
                return None;
            }
            let pc = bits.trailing_zeros() as u8;
            bits &= bits - 1;
            Some(PitchClass(pc))
        })
    }
}

impl FromIterator<PitchClass> for PcSet {
    fn from_iter<I: IntoIterator<Item = PitchClass>>(pcs: I) -> PcSet {
        pcs.into_iter().fold(PcSet::EMPTY, PcSet::with)
    }
}

impl fmt::Display for PcSet {
    /// The pitch classes by name, from C up: "C E G".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for pc in self.iter() {
            if !first {
                f.write_str(" ")?;
            }
            first = false;
            write!(f, "{pc}")?;
        }
        Ok(())
    }
}

/// Twelve values, one for each pitch class, picked by [`PitchClass`] (so a pick can never miss).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PerPitch<T>(pub [T; 12]);

impl<T> std::ops::Index<PitchClass> for PerPitch<T> {
    type Output = T;
    // A pitch class is always 0 to 11.
    #[allow(clippy::indexing_slicing)]
    fn index(&self, pc: PitchClass) -> &T {
        &self.0[pc.index()]
    }
}

impl<T> std::ops::IndexMut<PitchClass> for PerPitch<T> {
    #[allow(clippy::indexing_slicing)]
    fn index_mut(&mut self, pc: PitchClass) -> &mut T {
        &mut self.0[pc.index()]
    }
}

impl<T: Copy> PerPitch<T> {
    /// The values with pitch class `root` first: `rotated(root)[i]` is the value `i` semitones above `root`.
    pub fn rotated(&self, root: PitchClass) -> PerPitch<T> {
        let mut out = self.0;
        out.rotate_left(root.index());
        PerPitch(out)
    }

    /// Each pitch class with its value, from C up.
    pub fn iter(&self) -> impl Iterator<Item = (PitchClass, T)> + '_ {
        PitchClass::ALL.into_iter().zip(self.0.iter().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_classes_go_round_the_octave() {
        assert_eq!(PitchClass::new(-1), PitchClass::B);
        assert_eq!(PitchClass::new(12), PitchClass::C);
        assert_eq!(PitchClass::new(-25), PitchClass::B);
        assert_eq!(PitchClass::of_key(61).number(), 1);
        assert_eq!(PitchClass::E.above(PitchClass::C), 4);
        assert_eq!(PitchClass::C.above(PitchClass::E), 8);
        assert_eq!(PitchClass::B.up(2), PitchClass(1));
        assert_eq!(PitchClass::ALL.map(|p| p.to_string()).join(" "), "C Db D Eb E F F# G Ab A Bb B");
    }

    #[test]
    fn sets_turn_into_intervals_and_back() {
        let c_major = PcSet::of(&[0, 4, 7]);
        assert_eq!(c_major.bits(), 0b1001_0001);
        let a_minor = PcSet::of(&[9, 12, 16]);
        assert_eq!(a_minor.to_string(), "C E A");
        assert_eq!(a_minor.above(PitchClass::A), PcSet::of(&[0, 3, 7]));
        assert_eq!(PcSet::of(&[0, 3, 7]).on(PitchClass::A), a_minor);
        assert_eq!(c_major.transposed(13), PcSet::of(&[1, 5, 8]));
        assert_eq!(c_major.transposed(-1), PcSet::of(&[11, 3, 6]));
        assert!(c_major.has(16) && !c_major.has(3));
        assert_eq!(c_major.iter().collect::<PcSet>(), c_major);
        assert_eq!((c_major.len(), PcSet::EMPTY.first(), a_minor.first()), (3, None, Some(PitchClass::C)));
        // Every set comes back from every turn.
        for bits in 0..4096u16 {
            for s in -13..13 {
                let set = PcSet::from_bits(bits);
                assert_eq!(set.transposed(s).transposed(-s), set);
                assert_eq!(set.transposed(s).len(), set.len());
            }
        }
    }

    #[test]
    fn per_pitch_values_turn_with_the_root() {
        let mut v = PerPitch([0u8; 12]);
        v[PitchClass::G] = 7;
        assert_eq!(v.rotated(PitchClass::C)[PitchClass::G], 7);
        assert_eq!(v.rotated(PitchClass::E)[PitchClass::new(3)], 7);
        assert_eq!(v.iter().filter(|(_, x)| *x > 0).map(|(p, _)| p).collect::<Vec<_>>(), [PitchClass::G]);
    }
}
