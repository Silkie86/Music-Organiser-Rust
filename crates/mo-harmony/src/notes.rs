//! The notes the engine hears: parts with a job (melody, harmony, bass), on a grid of beats and bars.
//!
//! The caller gives the notes as they sound: transposed as the DAW plays them (an FL channel's root note, a clip's
//! transpose), muted notes left out. Times are in ticks; the grid says how many ticks make a beat and how many beats a
//! bar, so 3/4, 6/8 and 12/8 are heard as they are written.

use crate::pitch::PitchClass;

/// One note: when it starts and how long it sounds (ticks), the MIDI key as heard, and how hard it was played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub start: i64,
    pub length: i64,
    /// MIDI key, 0 to 127, at the pitch heard.
    pub key: u8,
    /// MIDI 2.0's 16-bit velocity, 0 to 65535 (rule 22): a MIDI 1.0 velocity is scaled up by the caller.
    pub velocity: u16,
}

impl Note {
    pub const fn pitch_class(self) -> PitchClass {
        PitchClass::of_key(self.key)
    }

    /// How long it sounds, in ticks: at least 1 (a damaged file's note of no length still sounds for a tick).
    pub const fn duration(self) -> i64 {
        if self.length > 0 { self.length } else { 1 }
    }

    /// Where it stops sounding.
    pub const fn end(self) -> i64 {
        self.start.saturating_add(self.duration())
    }
}

/// What a part does in the music, as far as the caller knows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Role {
    /// The tune: a lead, a vocal line, a topline, a counter-melody.
    Melody,
    /// Chords: keys, pads, comping, an arpeggio. It may hold the bass too, as a piano's left hand does.
    Harmony,
    /// A bass part.
    Bass,
    /// Drums and other unpitched parts, which the engine doesn't hear.
    Drums,
    /// Not known: heard as harmony.
    #[default]
    Unknown,
}

impl Role {
    /// Whether the part's notes are heard at all.
    pub const fn pitched(self) -> bool {
        !matches!(self, Role::Drums)
    }
}

/// One part (a track, a channel, a clip) and its notes, in any order.
#[derive(Clone, Copy, Debug)]
pub struct Part<'a> {
    pub role: Role,
    pub notes: &'a [Note],
}

/// The grid of beats and bars the notes sit on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    /// Ticks in a beat, the beat being the time signature's own (a quarter in 4/4, an eighth in 6/8).
    pub ticks_per_beat: u32,
    pub beats_per_bar: u32,
    /// A tick where a bar starts (any bar line: the grid repeats both ways from it).
    pub bar_line: i64,
}

impl Grid {
    /// Four beats a bar, bars starting at tick 0.
    pub const fn four_four(ticks_per_beat: u32) -> Grid {
        Grid { ticks_per_beat, beats_per_bar: 4, bar_line: 0 }
    }

    /// Ticks in a beat, at least 1.
    pub const fn beat(self) -> i64 {
        if self.ticks_per_beat > 0 { self.ticks_per_beat as i64 } else { 1 }
    }

    /// Beats in a bar, at least 1.
    pub const fn bar_beats(self) -> i64 {
        if self.beats_per_bar > 0 { self.beats_per_bar as i64 } else { 1 }
    }

    /// Ticks in a bar.
    pub const fn bar(self) -> i64 {
        self.beat().saturating_mul(self.bar_beats())
    }

    /// Whether the beat starting at this tick starts a bar.
    pub const fn starts_bar(self, tick: i64) -> bool {
        tick.saturating_sub(self.bar_line).rem_euclid(self.bar()) == 0
    }
}
