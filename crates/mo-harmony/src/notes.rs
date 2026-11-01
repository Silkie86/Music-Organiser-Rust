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

/// The velocity nine in ten of some notes are played at or below: the smallest velocity with at least 90% of the
/// notes at or below it, so a few loud accents don't move it. Doubling every note (an octave up) leaves it where it is,
/// which a rank like 0.9 (n - 1) doesn't. 0 for no notes. `velocities` is working space, emptied and filled here.
pub fn usual_velocity<'a>(notes: impl IntoIterator<Item = &'a Note>, velocities: &mut Vec<u16>) -> u32 {
    velocities.clear();
    velocities.extend(notes.into_iter().map(|n| n.velocity));
    let n = velocities.len();
    if n == 0 {
        return 0;
    }
    // ceil(0.9 n) - 1 from the quietest.
    let rank = (n.saturating_mul(9).saturating_add(9) / 10).saturating_sub(1).min(n - 1);
    let (_, v, _) = velocities.select_nth_unstable(rank);
    u32::from(*v)
}

/// Whether a note is a ghost note, a stray touch of a key rather than part of the music: shorter than an eighth of a
/// beat (`beat` ticks), and played at under an eighth of the usual velocity of its stream ([`usual_velocity`]). The key
/// finder and the chord reader both leave ghost notes out. This is the only rule that reads the 16-bit velocity, so
/// MIDI 2.0's resolution is never cut (rule 22).
pub fn is_ghost(note: &Note, beat: i64, usual: u32) -> bool {
    i128::from(note.duration()) * 8 < i128::from(beat) && u32::from(note.velocity) * 8 < usual
}

/// The start of the stretch `span` ticks long that holds the most of these onsets (the earliest, if two hold as many):
/// where the music is, when a damaged file has notes far before or after it. Sorts `starts`. `None` for no onsets.
pub(crate) fn busiest(starts: &mut [i64], span: i128) -> Option<i128> {
    starts.sort_unstable();
    let (mut best, mut most, mut end) = (None, 0, 0);
    for (i, &s) in starts.iter().enumerate() {
        while starts.get(end).is_some_and(|&t| i128::from(t) - i128::from(s) < span) {
            end += 1;
        }
        if end - i > most {
            (best, most) = (Some(i128::from(s)), end - i);
        }
    }
    best
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

#[cfg(test)]
mod tests {
    use super::*;

    fn note(length: i64, velocity: u16) -> Note {
        Note { start: 0, length, key: 60, velocity }
    }

    #[test]
    fn the_usual_velocity_is_the_one_nine_in_ten_notes_are_played_at_or_below() {
        let mut scratch = Vec::new();
        let notes: Vec<Note> = (1..=10).map(|v| note(480, v * 100)).collect();
        assert_eq!(usual_velocity(&notes, &mut scratch), 900);
        // Doubling every note keeps it; one note is its own; none is 0.
        let doubled: Vec<Note> = notes.iter().chain(&notes).copied().collect();
        assert_eq!(usual_velocity(&doubled, &mut scratch), 900);
        assert_eq!(usual_velocity(&notes[..1], &mut scratch), 100);
        assert_eq!(usual_velocity(&[], &mut scratch), 0);
        // 19 quiet notes and one loud accent: the accent doesn't move it.
        let accent: Vec<Note> = (0..20).map(|i| note(480, if i == 0 { 60000 } else { 4000 })).collect();
        assert_eq!(usual_velocity(&accent, &mut scratch), 4000);
    }

    #[test]
    fn a_ghost_note_is_both_short_and_quiet() {
        // An eighth of a 480-tick beat is 60 ticks; an eighth of a usual 8000 is 1000.
        assert!(is_ghost(&note(59, 999), 480, 8000));
        assert!(!is_ghost(&note(60, 999), 480, 8000), "long enough");
        assert!(!is_ghost(&note(59, 1000), 480, 8000), "loud enough");
        // A note of no length sounds for a tick; nothing is a ghost when nothing is usual.
        assert!(is_ghost(&note(-5, 10), 480, 8000));
        assert!(!is_ghost(&note(1, 0), 480, 0));
    }

    #[test]
    fn the_busiest_stretch_starts_where_most_notes_are() {
        let mut starts = vec![0, 10_000, 10_100, 10_200, -5_000_000, 20_000];
        assert_eq!(busiest(&mut starts, 1_000), Some(10_000));
        assert_eq!(busiest(&mut [], 1_000), None);
        // Ties go to the earliest.
        assert_eq!(busiest(&mut [500, 0], 100), Some(0));
    }
}
