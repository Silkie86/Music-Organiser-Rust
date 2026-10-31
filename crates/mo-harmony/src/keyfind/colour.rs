//! The colour of a key ([`Colours`]): Mixolydian, blues, Dorian, harmonic minor, or a minor with no sixth. Read after
//! the key, from how long each note sounds counted up from its tonic; a colour never changes the key.
//!
//! The rules are set by hand, in whole numbers, so they move with the notes exactly. They're among the numbers the
//! finder's version hash covers (`shipped_hash`): changing one changes answers.

use crate::key::{Key, Mode};
use crate::pitch::PitchClass;

use super::{Heard, KeyAnswer};

/// Colours of a key, as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Colours(u8);

impl Colours {
    pub const NONE: Colours = Colours(0);
    /// A major key with its flat seventh and not its major seventh: C with Bb.
    pub const MIXOLYDIAN: Colours = Colours(1);
    /// With [`Colours::MIXOLYDIAN`]: the flat seventh sounds over the tonic in the bass (C7 as home).
    pub const I7: Colours = Colours(2);
    /// A major key with both thirds and the flat seventh: C with Eb, E and Bb.
    pub const BLUES: Colours = Colours(4);
    /// A minor key with its major sixth and not its minor sixth: C minor with A.
    pub const DORIAN: Colours = Colours(8);
    /// A minor key with its raised seventh: C minor with B.
    pub const HARMONIC_MINOR: Colours = Colours(16);
    /// A minor key with no sixth at all, so Aeolian and Dorian can't be told apart.
    pub const NO_SIXTH: Colours = Colours(32);
    /// Every bit a colour uses.
    const ALL_BITS: u8 = 63;

    /// The colours these bits hold (bits no colour uses are dropped).
    pub const fn from_bits(bits: u8) -> Colours {
        Colours(bits & Colours::ALL_BITS)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn has(self, colour: Colours) -> bool {
        self.0 & colour.0 == colour.0 && colour.0 != 0
    }

    pub const fn with(self, colour: Colours) -> Colours {
        Colours(self.0 | colour.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A share as a fraction, numerator then denominator: the rules compare whole numbers, never rounded ones.
type Share = (u128, u128);

/// A colour note must sound for at least this share of the notes' time (4%).
pub(super) const COLOUR_NOTE: Share = (1, 25);
/// The raised seventh of harmonic minor needs at least this share (3%), and at least half the flat seventh's.
pub(super) const LEADING_NOTE: Share = (3, 100);
/// No sixth: the two sixths together under this share (2%).
pub(super) const NO_SIXTH_BELOW: Share = (1, 50);
/// I7: the flat seventh sounds over the tonic in the bass on at least this share of the tonic's bass beats (20%). A
/// gospel turn (C, C7, F, Fm, C, C) has it on a quarter of them; a pop song's passing C7 on fewer.
pub(super) const I7_SHARE: Share = (1, 5);

/// The colours of the key an answer gives or leans to: a home note with no leaning is read as major (where the blues
/// lives); no clear key has none.
pub fn colours(heard: &Heard, answer: KeyAnswer) -> Colours {
    let key = match answer {
        KeyAnswer::Key(k) | KeyAnswer::OrRelative(k) => k,
        KeyAnswer::HomeNote { tonic, leaning } => Key { tonic, mode: leaning.unwrap_or(Mode::Major) },
        KeyAnswer::Unclear { .. } => return Colours::NONE,
    };
    of_key(heard, key)
}

/// The colours of `key` in what was heard.
pub fn of_key(heard: &Heard, key: Key) -> Colours {
    let t = key.tonic;
    let time = |d: i32| {
        let pc = t.up(d);
        u128::from(heard.tune[pc]) + u128::from(heard.harmony[pc])
    };
    let total: u128 =
        PitchClass::ALL.iter().map(|pc| u128::from(heard.tune[*pc]) + u128::from(heard.harmony[*pc])).sum();
    if total == 0 {
        return Colours::NONE;
    }
    // Whether note `d` above the tonic sounds for at least this share of the time.
    let at_least = |d: i32, (num, den): Share| time(d) * den >= num * total;
    let mut colours = Colours::NONE;
    match key.mode {
        Mode::Major => {
            if at_least(3, COLOUR_NOTE) && at_least(4, COLOUR_NOTE) && at_least(10, COLOUR_NOTE) {
                colours = colours.with(Colours::BLUES);
            } else if at_least(10, COLOUR_NOTE) && time(10) >= 2 * time(11) {
                colours = colours.with(Colours::MIXOLYDIAN);
                let (num, den) = I7_SHARE;
                let (flat7, bass) = (u128::from(heard.flat7_on_bass[t]), u128::from(heard.bass[t]));
                if bass > 0 && flat7 * den >= num * bass {
                    colours = colours.with(Colours::I7);
                }
            }
        }
        Mode::Minor => {
            if at_least(11, LEADING_NOTE) && 2 * time(11) >= time(10) {
                colours = colours.with(Colours::HARMONIC_MINOR);
            }
            if at_least(9, COLOUR_NOTE) && time(9) >= 2 * time(8) {
                colours = colours.with(Colours::DORIAN);
            }
            let (num, den) = NO_SIXTH_BELOW;
            if (time(8) + time(9)) * den < num * total {
                colours = colours.with(Colours::NO_SIXTH);
            }
        }
    }
    colours
}
