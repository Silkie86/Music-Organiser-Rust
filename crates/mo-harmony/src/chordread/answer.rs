//! The answer a person would give for each span (harmony/design/chord-reader-design.md, "Calibration, the graded answer
//! and alternatives for Chord check"): one chord, one set of notes written more than one way, a chord only implied by
//! one line, not sure, or no chord.
//!
//! The answer is graded from the span's readings, in this order ([`grade`]):
//!
//! 1. **No chord, a rest:** silence of a bar or more.
//! 2. **A chord given** (a pin with its chord, a Chord check answer or the user's fix): one chord, as given.
//! 3. **No chord, too few notes:** fewer than two notes heard.
//! 4. **Implied:** only one line is heard ([`Texture::SingleLine`]): a tune, a bass line, or a part played one note at a
//!    time. It leans to the first reading, and is never one chord.
//! 5. **One chord:** the first reading, the one the best way reads, has at least the chance [`Thresholds::one`] asks
//!    for. Never a rootless reading with no bass: without a bass, nothing says which root a rootless voicing stands on.
//! 6. **The same notes:** the first reading's same-notes group (C6 or Am7; C♯dim7 or A7(♭9)), two readings or more,
//!    adds up to at least [`Thresholds::same`].
//! 7. **Unclear:** otherwise, leaning to the first reading.
//!
//! The thresholds are set by hand for now ([`super::THRESHOLDS`]): the calibration's fit will set them, each as the
//! least chance at which that kind of answer is wrong at most one time in ten.

use super::hear::{Hearing, Stream};

/// What kinds of line are heard: what the notes can say, and so how sure the reader may be of its chords.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Texture {
    /// A tune and other parts (chords, or a bass part).
    Full,
    /// Chords and a bass part, with no tune.
    Accompaniment,
    /// Chords alone.
    HarmonyOnly,
    /// One line alone: a tune, a bass part, or chords played one note at a time and never together (a lead, never an
    /// arpeggio: [`Hearing::one_line`]).
    SingleLine,
}

impl Texture {
    pub const ALL: [Texture; 4] = [Texture::Full, Texture::Accompaniment, Texture::HarmonyOnly, Texture::SingleLine];

    /// Its place in [`Texture::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The texture of what was heard: which streams have notes, and for chords alone, whether they're one line.
    pub fn of(hearing: &Hearing) -> Texture {
        let (mut tune, mut bass, mut chords) = (false, false, false);
        for n in hearing.notes() {
            match n.stream {
                Stream::Melody => tune = true,
                Stream::Bass => bass = true,
                Stream::Harmony => chords = true,
            }
        }
        match (tune, bass, chords) {
            (true, false, false) | (false, true, false) => Texture::SingleLine,
            (true, _, _) => Texture::Full,
            (false, true, true) => Texture::Accompaniment,
            (false, false, _) if hearing.one_line() => Texture::SingleLine,
            (false, false, _) => Texture::HarmonyOnly,
        }
    }
}

/// The answer for a span: how sure the reader is of its chord, as a person would say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChordAnswer {
    /// One chord: the first reading.
    Chord,
    /// One set of notes that can be written more than one way: the first reading's same-notes group (C6 or Am7, a
    /// diminished seventh or the rootless 7(♭9) it stands for).
    SameNotes,
    /// Only one line is heard: the chord is implied, leaning to the first reading.
    Implied,
    /// Not sure: leaning to the first reading, when there is one.
    Unclear,
    /// No chord.
    NoChord(NoChord),
}

/// Why a span has no chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoChord {
    /// Silent for a bar or more.
    Rest,
    /// Fewer than two notes heard.
    TooFewNotes,
}

impl ChordAnswer {
    /// Every answer, in the order the fact numbers them (0 to 5).
    pub const ALL: [ChordAnswer; 6] = [
        ChordAnswer::Chord,
        ChordAnswer::SameNotes,
        ChordAnswer::Implied,
        ChordAnswer::Unclear,
        ChordAnswer::NoChord(NoChord::Rest),
        ChordAnswer::NoChord(NoChord::TooFewNotes),
    ];

    /// Whether it gives a chord to show: every answer but no chord (a chord implied or unclear leans to one).
    pub const fn shows_chord(self) -> bool {
        !matches!(self, ChordAnswer::NoChord(_))
    }
}

/// How sure a span's readings must be for each kind of answer, for each texture (in [`Texture::ALL`]'s order). No
/// threshold is below a half, and 1 or more means that kind of answer is never given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// The least chance of the first reading for one chord ([`ChordAnswer::Chord`]).
    pub one: [f64; 4],
    /// The least the first reading's same-notes group adds up to for the same notes ([`ChordAnswer::SameNotes`]).
    pub same: [f64; 4],
}

impl Thresholds {
    /// The thresholds for a texture.
    pub fn at(&self, texture: Texture) -> (f64, f64) {
        let at = |t: &[f64; 4]| t.get(texture.index()).copied().unwrap_or(f64::INFINITY);
        (at(&self.one), at(&self.same))
    }
}

/// What a span's answer is graded from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grading {
    /// Whether it's a rest: silent for a bar or more.
    pub rest: bool,
    /// Whether its chord was given (a pin with its chord).
    pub given: bool,
    /// How many notes are heard over it (pitch classes).
    pub heard: u32,
    /// Whether the best way reads a chord there (not no chord), so it's the first reading.
    pub decoded: bool,
    /// The first reading's chance, and whether it's rootless with no bass.
    pub first: f64,
    pub rootless_with_no_bass: bool,
    /// How many readings are in the first reading's same-notes group, and their chances added up.
    pub group: usize,
    pub group_chance: f64,
    pub texture: Texture,
}

/// The answer for a span ([`ChordAnswer`]), by the rules in order (see the module's notes).
pub fn grade(g: Grading, thresholds: &Thresholds) -> ChordAnswer {
    let (one, same) = thresholds.at(g.texture);
    // A threshold below a half counts as a half, and one of 1 or more is never met (a chance can round up to 1).
    let meets = |chance: f64, threshold: f64| threshold < 1.0 && chance >= threshold.max(0.5);
    if g.rest {
        ChordAnswer::NoChord(NoChord::Rest)
    } else if g.given {
        ChordAnswer::Chord
    } else if g.heard < 2 {
        ChordAnswer::NoChord(NoChord::TooFewNotes)
    } else if g.texture == Texture::SingleLine {
        ChordAnswer::Implied
    } else if g.decoded && !g.rootless_with_no_bass && meets(g.first, one) {
        ChordAnswer::Chord
    } else if g.decoded && g.group >= 2 && meets(g.group_chance, same) {
        ChordAnswer::SameNotes
    } else {
        ChordAnswer::Unclear
    }
}
