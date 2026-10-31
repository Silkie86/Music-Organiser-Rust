//! A reading in eight bytes ([`KeyFact`]), for the MIDI page's index to keep beside each part and search by. It holds
//! the whole answer, so the page can show it without reading the notes again.

use crate::key::{Key, Mode};
use crate::pitch::PitchClass;

use super::{Colours, KeyAnswer, KeyReading, Why};

/// A reading in eight bytes:
///
/// - `[0]` the layout, 1 ([`KeyFact::LAYOUT`]);
/// - `[1]` the kind of answer: 0 one key, 1 a key or its relative, 2 a home note leaning to a mode, 6 a home note with
///   both modes as likely, 3 too few notes, 4 not tonal, 5 spread;
/// - `[2]` a key's index in [`Key::ALL`] (255 for none): the key; the likelier of a pair; the home note in the mode it
///   leans to, or its major key when it leans to neither; the key no clear key leans to;
/// - `[3]` the runner-up's index (255 for none);
/// - `[4..6]` the chance shown with the answer, times 65,535, high byte first;
/// - `[6]` the likeliest key's chance, times 255;
/// - `[7]` the colour bits ([`Colours`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyFact(pub [u8; 8]);

/// No key, in a [`KeyFact`].
const NONE: u8 = 255;

impl KeyFact {
    /// The layout this version writes.
    pub const LAYOUT: u8 = 1;

    /// The fact of a reading.
    pub fn of(reading: &KeyReading) -> KeyFact {
        let index = |k: Option<Key>| k.map_or(NONE, |k| k.index() as u8);
        let (kind, key) = match reading.answer {
            KeyAnswer::Key(k) => (0, Some(k)),
            KeyAnswer::OrRelative(k) => (1, Some(k)),
            KeyAnswer::HomeNote { tonic, leaning: Some(mode) } => (2, Some(Key { tonic, mode })),
            KeyAnswer::HomeNote { tonic, leaning: None } => (6, Some(Key::major(tonic))),
            KeyAnswer::Unclear { why, leaning } => (
                match why {
                    Why::TooFewNotes => 3,
                    Why::NotTonal => 4,
                    Why::Spread => 5,
                },
                leaning,
            ),
        };
        let shown = (reading.shown_chance.clamp(0.0, 1.0) * 65_535.0).round() as u16;
        let best = (reading.best().1.clamp(0.0, 1.0) * 255.0).round() as u8;
        let [high, low] = shown.to_be_bytes();
        KeyFact([
            KeyFact::LAYOUT,
            kind,
            index(key),
            index(reading.runner_up().map(|(k, _)| k)),
            high,
            low,
            best,
            reading.colours.bits(),
        ])
    }

    fn key_at(&self, byte: usize) -> Option<Key> {
        self.0.get(byte).and_then(|i| Key::ALL.get(usize::from(*i))).copied()
    }

    /// The answer it holds; `None` for a fact of another layout, or a damaged one.
    pub fn answer(&self) -> Option<KeyAnswer> {
        let [layout, kind, ..] = self.0;
        if layout != KeyFact::LAYOUT {
            return None;
        }
        let key = self.key_at(2);
        Some(match kind {
            0 => KeyAnswer::Key(key?),
            1 => KeyAnswer::OrRelative(key?),
            2 => KeyAnswer::HomeNote { tonic: key?.tonic, leaning: Some(key?.mode) },
            6 => KeyAnswer::HomeNote { tonic: key?.tonic, leaning: None },
            3 => KeyAnswer::Unclear { why: Why::TooFewNotes, leaning: key },
            4 => KeyAnswer::Unclear { why: Why::NotTonal, leaning: key },
            5 => KeyAnswer::Unclear { why: Why::Spread, leaning: key },
            _ => return None,
        })
    }

    /// The home note, for search: a key's, the likelier of a pair's, or the home note. `None` when the key isn't clear.
    pub fn tonic(&self) -> Option<PitchClass> {
        self.answer()?.tonic()
    }

    /// Major or minor, for search: only when the answer gives one key, or a pair (the likelier's mode).
    pub fn mode(&self) -> Option<Mode> {
        match self.answer()? {
            KeyAnswer::Key(k) | KeyAnswer::OrRelative(k) => Some(k.mode),
            KeyAnswer::HomeNote { .. } | KeyAnswer::Unclear { .. } => None,
        }
    }

    /// The runner-up key.
    pub fn runner_up(&self) -> Option<Key> {
        self.key_at(3)
    }

    /// The chance shown with the answer, to within 1/65,535.
    pub fn shown_chance(&self) -> f64 {
        let [_, _, _, _, high, low, _, _] = self.0;
        f64::from(u16::from_be_bytes([high, low])) / 65_535.0
    }

    /// The likeliest key's chance, to within 1/255.
    pub fn best_chance(&self) -> f64 {
        let [.., best, _] = self.0;
        f64::from(best) / 255.0
    }

    pub fn colours(&self) -> Colours {
        let [.., colours] = self.0;
        Colours::from_bits(colours)
    }
}
