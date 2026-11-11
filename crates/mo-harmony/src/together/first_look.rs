//! A first look, to pick which parts of a library are worth the close look ([`sit`](super::sit)).
//!
//! The close look needs a part's notes, and a library keeps those on disk: trying every part would be tens of
//! thousands of reads. So a library keeps a little of each part in memory, a [`Slot`] for every two beats (how long
//! the part plays each pitch class in them, 12 bytes), and the first look sorts every part by those alone: a tune
//! over the chords something is fitted to ([`Backing::outline`], then [`Outline::sits`]), or chords under a tune
//! ([`Outline::guessed`] from a chord part's slots, with no chord read in them). Only the best share then gets the
//! close look.
//!
//! How long a slot is, in ticks, is the caller's to say, and the same for everything looked at together: two
//! quarter notes, half a bar of 4/4, is what was measured. (A [`Grid`]'s beat is the time signature's own, an
//! eighth in 6/8, so two of its beats wouldn't be the same stretch of time in every song.)
//!
//! How much it may leave out was measured (`theorybench pairs --quick`; `docs/harmony/together-experiments.md` has
//! the numbers): for a tune over chords, keeping the best 2 parts in 100 keeps 99 in 100 of the close look's best
//! ten; for chords under a tune, guessed, it takes the best 10 in 100 to keep 98. What a library held before, how
//! long a part plays each pitch class over all of it with no timing, doesn't do: the best 5 in 100 by that keep 7
//! in 100 of the close look's best ten tunes and 16 in 100 of its best ten chord parts.

use super::{
    ABOVE, ALIKE, BESIDE, Backing, COLOUR, FIFTH, LEAST_HEARD, MOST_BARS_ROUND, NEAR_PLAYED, NO_CHORD, OUTSIDE,
    PLAYED_SHARE, RARE, ROOT, Sits, Sitting, THIRD,
};
use crate::notes::{Grid, Note};
use crate::pitch::PitchClass;

/// How long a part plays each pitch class (from C up) in one slot of time, two quarter notes as a rule:
/// [`SLOT_FULL`] for all of it. The same note in two octaves counts twice, up to 255.
pub type Slot = [u8; 12];

/// A pitch class played all through a [`Slot`].
pub const SLOT_FULL: u8 = 64;

/// The most slots kept of a part or looked at together: the 32 bars two loops are played round for at the most.
pub const MOST_SLOTS: usize = 2 * MOST_BARS_ROUND as usize;

/// A part's slots, each `slot` ticks long, from the bar its first note is in to the bar its last note ends in, as
/// the close look hears a loop go round ([`LineUp::Loops`](super::LineUp::Loops)). Its first [`MOST_SLOTS`] at the
/// most; none for no notes.
pub fn slots_of(notes: &[Note], grid: Grid, slot: i64) -> Vec<Slot> {
    let (Some(first), Some(last)) = (notes.iter().map(|n| n.start).min(), notes.iter().map(|n| n.end()).max()) else {
        return Vec::new();
    };
    let bar = grid.bar();
    let from = first.saturating_sub(grid.bar_line).div_euclid(bar).saturating_mul(bar).saturating_add(grid.bar_line);
    let length = last.saturating_sub(from).saturating_add(bar - 1).div_euclid(bar).max(1).saturating_mul(bar);
    slots_over(notes, slot, from, length)
}

/// A part's slots, each `slot` ticks long, over `length` ticks from `from`, a bar line (bars picked from a song: one
/// the part rests in is still a slot or two). What it plays outside them is left out. The first [`MOST_SLOTS`] at
/// the most.
pub fn slots_over(notes: &[Note], slot: i64, from: i64, length: i64) -> Vec<Slot> {
    let slot = slot.max(1);
    let length = length.max(0);
    let count = usize::try_from(length.saturating_add(slot - 1) / slot).unwrap_or(0).min(MOST_SLOTS);
    let mut times = vec![[0i64; 12]; count];
    let end = (count as i64).saturating_mul(slot).min(length);
    for note in notes {
        let (start, stop) = (note.start.saturating_sub(from).max(0), note.end().saturating_sub(from).min(end));
        let mut at = start;
        while at < stop {
            let k = at / slot;
            let until = (k + 1).saturating_mul(slot).min(stop);
            if let Some(time) = usize::try_from(k)
                .ok()
                .and_then(|k| times.get_mut(k))
                .and_then(|t| t.get_mut(note.pitch_class().index()))
            {
                *time = time.saturating_add(until - at);
            }
            at = until;
        }
    }
    times
        .iter()
        .map(|time| {
            time.map(|t| {
                // To the nearest part of a slot, but never to nothing: a grace note is still a note.
                let parts = (t.saturating_mul(i64::from(SLOT_FULL)).saturating_add(slot / 2) / slot).min(255);
                if t > 0 { parts.max(1) as u8 } else { 0 }
            })
        })
        .collect()
}

/// Chords as a first look hears them: what a note of each pitch class is worth over them in each slot.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    /// For each slot, from C up; none where nothing is heard.
    worth: Vec<Option<[f64; 12]>>,
    /// How much of the chords' sounding time each pitch class takes, the most-played 1.
    scale: [f64; 12],
    /// What being alike is worth ([`Sitting`]'s `alike`).
    alike: f64,
}

impl Backing {
    /// The backing as a first look hears it, in slots of `slot` ticks and with these numbers: over each slot, what
    /// the close look gives a note of each pitch class there, averaged over the slot. A tune's slots are then tried
    /// against it in about a microsecond ([`Outline::sits`]).
    pub fn outline(&self, slot: i64, how: &Sitting) -> Outline {
        let worths = how.worths();
        let worth_of = |place: usize| worths.get(place).copied().unwrap_or(0.0);
        let (slot, half) = (slot.max(1), self.half());
        let count = usize::try_from(self.length.saturating_add(slot - 1) / slot).unwrap_or(0).min(MOST_SLOTS);
        let mut worth = Vec::with_capacity(count);
        for k in 0..count as i64 {
            let to = (k + 1).saturating_mul(slot).min(self.length);
            let mut sums = [0.0f64; 12];
            let mut heard = 0i64;
            // Each chord and each half bar in the slot in turn, as the close look walks them under a note.
            let mut at = k.saturating_mul(slot);
            while at < to {
                let s = self.stretches.partition_point(|s| s.end <= at);
                let stretch = self.stretches.get(s).filter(|s| s.start <= at);
                let until = match stretch {
                    Some(s) => s.end,
                    None => self.stretches.get(s).map_or(self.length, |s| s.start),
                }
                .min(to)
                .min((at / half + 1).saturating_mul(half))
                .max(at + 1);
                let near = usize::try_from(at / half).ok().and_then(|h| self.halves.get(h).copied().flatten());
                let time = until - at;
                at = until;
                // Where there's neither a chord nor sound, a note isn't heard.
                if stretch.is_none() && near.is_none() {
                    continue;
                }
                heard += time;
                for (pc, sum) in PitchClass::ALL.into_iter().zip(&mut sums) {
                    let mut worth = worth_of(stretch.map_or(NO_CHORD, |s| s.kinds[pc]));
                    if let Some(near) = near {
                        let near = near[pc];
                        if near.played {
                            worth += worth_of(NEAR_PLAYED);
                        }
                        for (d, strength) in near.above.iter().enumerate().filter(|(_, s)| **s > 0.0) {
                            worth += strength * worth_of(ABOVE + d);
                        }
                    }
                    *sum += time as f64 * worth;
                }
            }
            worth.push((heard > 0).then(|| sums.map(|sum| sum / heard as f64)));
        }
        Outline { worth, scale: self.scale.0, alike: worth_of(ALIKE) }
    }
}

impl Outline {
    /// Chords guessed from a chord part's slots alone, with no chord read in them: a note played in a slot is taken
    /// for a note of the chord there, one that isn't for a note beside it, rubbing the ones that are as the close
    /// look counts it. For trying chord parts under a tune without reading each one's chords first.
    pub fn guessed(slots: &[Slot], how: &Sitting) -> Outline {
        let worths = how.worths();
        let worth_of = |place: usize| worths.get(place).copied().unwrap_or(0.0);
        let slots = slots.get(..MOST_SLOTS).unwrap_or(slots);
        // The same note in two octaves is one note of the chord: no pitch class counts for more than the whole
        // slot. (Measured: without this the guess keeps 4 in 100 fewer of the close look's best.)
        let heard = |slot: &Slot| slot.map(|t| t.min(SLOT_FULL));
        let mut whole = [0u32; 12];
        for slot in slots {
            for (sum, time) in whole.iter_mut().zip(heard(slot)) {
                *sum += u32::from(time);
            }
        }
        let most = whole.iter().copied().max().unwrap_or(0);
        let scale = if most > 0 { whole.map(|t| f64::from(t) / f64::from(most)) } else { [0.0; 12] };
        // A played note could be any note of the chord.
        let played_worth =
            (worth_of(ROOT) + worth_of(THIRD) + worth_of(FIFTH) + worth_of(COLOUR)) / 4.0 + worth_of(NEAR_PLAYED);
        let worth = slots
            .iter()
            .map(|slot| {
                let slot = heard(slot);
                let most = slot.iter().copied().max().filter(|m| *m > 0)?;
                let strength = slot.map(|t| f64::from(t) / f64::from(most));
                let at = |pc: usize| strength.get(pc % 12).copied().unwrap_or(0.0);
                let mut out = [0.0f64; 12];
                for (pc, (cell, in_scale)) in out.iter_mut().zip(&scale).enumerate() {
                    *cell = if at(pc) >= PLAYED_SHARE {
                        played_worth
                    } else {
                        let mut off = worth_of(if *in_scale >= RARE { BESIDE } else { OUTSIDE });
                        // The played notes it's 1 to 11 semitones above.
                        for d in 0..11usize {
                            let below = at(pc + 11 - d);
                            if below >= PLAYED_SHARE {
                                off += below * worth_of(ABOVE + d);
                            }
                        }
                        off
                    };
                }
                Some(out)
            })
            .collect();
        Outline { worth, scale, alike: worth_of(ALIKE) }
    }

    /// How a tune is likely to sit over the chords, from its slots alone, moved up by each of 0 to 11 semitones:
    /// both go round from their first slot until they meet again, as two loops do in the close look. The tune's
    /// slots must be as long as the chords'. A guess to sort by: the close look ([`sit`](super::sit)) gives the
    /// answer. None for a tune with no notes, or one that hardly plays where the chords have sound.
    pub fn sits(&self, tune: &[Slot]) -> Option<Sits> {
        let tune = tune.get(..MOST_SLOTS).unwrap_or(tune);
        let (mine, theirs) = (tune.len(), self.worth.len());
        if mine == 0 || theirs == 0 {
            return None;
        }
        // Until they meet again, or once through the longer when that's past the most looked at.
        let together = mine / gcd(mine, theirs) * theirs;
        let round = if together <= MOST_SLOTS { together } else { mine.max(theirs) };
        let mut sums = [0.0f64; 12];
        let mut tune_time = [0.0f64; 12];
        let (mut heard, mut all) = (0.0f64, 0.0f64);
        for k in 0..round {
            let (Some(slot), Some(worth)) = (tune.get(k % mine), self.worth.get(k % theirs)) else { continue };
            let time: u32 = slot.iter().map(|t| u32::from(*t)).sum();
            all += f64::from(time);
            let Some(worth) = worth else { continue };
            heard += f64::from(time);
            for (pc, time) in slot.iter().enumerate().filter(|(_, t)| **t > 0) {
                let time = f64::from(*time);
                if let Some(sum) = tune_time.get_mut(pc) {
                    *sum += time;
                }
                for (s, sum) in sums.iter_mut().enumerate() {
                    *sum += time * worth.get((pc + s) % 12).copied().unwrap_or(0.0);
                }
            }
        }
        if all <= 0.0 || heard < LEAST_HEARD * all {
            return None;
        }
        // The tune as a whole beside the chords as a whole.
        let (own, other): (f64, f64) = (tune_time.iter().map(|t| t * t).sum(), self.scale.iter().map(|t| t * t).sum());
        let mut by_move = [0.0f64; 12];
        for (s, (sits, sum)) in by_move.iter_mut().zip(&sums).enumerate() {
            let on: f64 = tune_time
                .iter()
                .enumerate()
                .map(|(pc, time)| time * self.scale.get((pc + s) % 12).copied().unwrap_or(0.0))
                .sum();
            let alike = if own > 0.0 && other > 0.0 { on / (own * other).sqrt() } else { 0.0 };
            *sits = sum / heard + self.alike * alike;
        }
        Some(Sits { by_move, heard: (heard / all).min(1.0) })
    }
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::chordread::ChordReader;
    use crate::keyfind::Span;
    use crate::notes::{Part, Role};
    use crate::together::{LineUp, sit};

    const BEAT: i64 = 96;
    const BAR: i64 = 4 * BEAT;
    /// Two quarter notes.
    const SLOT: i64 = 2 * BEAT;

    fn slots(notes: &[Note]) -> Vec<Slot> {
        slots_of(notes, grid(), SLOT)
    }

    fn grid() -> Grid {
        Grid::four_four(BEAT as u32)
    }

    fn note(start: i64, length: i64, key: u8) -> Note {
        Note { start, length, key, velocity: 40_000 }
    }

    /// Am7, Dm7, G7, Cmaj7, a bar each, root in the bass, moved up by `up` semitones.
    fn chords(up: u8) -> Vec<Note> {
        [[45, 60, 64, 67], [38, 60, 65, 69], [43, 59, 62, 65], [36, 59, 64, 67]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, bar)| keys.iter().map(move |&k| note(bar * BAR, BAR, k + up)))
            .collect()
    }

    /// Two notes a bar, half a bar each, on the chords' thirds and sevenths: C G, F C, B F, E B.
    fn tune(up: u8) -> Vec<Note> {
        [72, 79, 77, 72, 71, 77, 76, 71]
            .iter()
            .zip(0i64..)
            .map(|(&k, at)| note(at * 2 * BEAT, 2 * BEAT, k + up))
            .collect()
    }

    fn backing(notes: &[Note]) -> Backing {
        let parts = [Part { role: Role::Harmony, notes }];
        Backing::read(&mut ChordReader::new(), &parts, grid(), Span::Loop).unwrap()
    }

    #[test]
    fn slots_hold_how_long_each_pitch_class_sounds_in_two_beats() {
        // C for a beat, E for three beats from beat 2 (into the next slot), and a C an octave up with the first.
        let bar = slots(&[note(0, BEAT, 60), note(0, BEAT, 72), note(BEAT, 3 * BEAT, 64)]);
        // One bar: two slots. Two Cs for a beat are a whole slot's worth of C; the E takes half of the first slot
        // and all of the second.
        assert_eq!(bar.len(), 2);
        assert_eq!((bar[0][0], bar[0][4], bar[1][4], bar[1][0]), (SLOT_FULL, SLOT_FULL / 2, SLOT_FULL, 0));
        // A part that starts in bar 3 starts at its own first bar; bars picked from a song keep their rests.
        let late: Vec<Note> = tune(0).into_iter().map(|n| Note { start: n.start + 2 * BAR, ..n }).collect();
        assert_eq!(slots(&late), slots(&tune(0)));
        let picked = slots_over(&late, SLOT, 0, 8 * BAR);
        assert_eq!(picked.len(), 16);
        assert!(picked[..4].iter().all(|s| *s == [0; 12]) && picked[4] != [0; 12]);
        assert!(picked[12..].iter().all(|s| *s == [0; 12]));
        // A grace note is still a note, and five octaves of one note don't overflow.
        assert_eq!(slots(&[note(0, 1, 61)])[0][1], 1);
        let stack: Vec<Note> = (0..9).map(|octave| note(0, 2 * BEAT, 12 * octave)).collect();
        assert_eq!(slots(&stack)[0][0], 255);
        assert!(slots(&[]).is_empty());
    }

    #[test]
    fn a_tune_of_whole_slots_sits_as_the_close_look_says() {
        // Notes that each fill a slot: the first look knows all the close look does.
        let backing = backing(&chords(0));
        let outline = backing.outline(SLOT, &Sitting::SHIPPED);
        for up in [0u8, 3, 5, 7] {
            let close = sit(&tune(up), &backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap();
            let first = outline.sits(&slots(&tune(up))).unwrap();
            for (a, b) in close.by_move.iter().zip(&first.by_move) {
                assert!((a - b).abs() < 1e-9, "{up}: {close:?} {first:?}");
            }
            assert_eq!(first.best().0, close.best().0);
            assert!((first.heard - close.heard).abs() < 1e-9);
        }
        // A tune half as long goes round twice over the chords, as in the close look.
        let half: Vec<Note> = tune(0).into_iter().take(4).collect();
        let close = sit(&half, &backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED).unwrap();
        let first = outline.sits(&slots(&half)).unwrap();
        assert!((close.moved(0) - first.moved(0)).abs() < 1e-9, "{close:?} {first:?}");
    }

    #[test]
    fn a_song_in_six_eight_is_looked_at_in_the_same_slots_as_a_part_read_in_four_four() {
        // Two bars of 6/8 at 48 ticks an eighth: A minor, then G. A bar is 288 ticks, a slot still two quarter notes
        // (192), so two bars are three slots, the middle one over the bar line.
        let six_eight = Grid { ticks_per_beat: 48, beats_per_bar: 6, bar_line: 0 };
        let bar = six_eight.bar();
        let chords: Vec<Note> = [[45, 57, 60, 64], [43, 55, 59, 62]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, at)| keys.iter().map(move |&k| note(at * bar, bar, k)))
            .collect();
        let parts = [Part { role: Role::Harmony, notes: &chords }];
        let backing = Backing::read(&mut ChordReader::new(), &parts, six_eight, Span::Loop).unwrap();
        let outline = backing.outline(SLOT, &Sitting::SHIPPED);
        // A tune from the library, read as 4/4 as every part there is: a note a slot, C, B, D.
        let tune: Vec<Note> = [72u8, 71, 74].iter().zip(0i64..).map(|(&k, at)| note(at * SLOT, SLOT, k)).collect();
        let first = outline.sits(&slots_over(&tune, SLOT, 0, 2 * bar)).unwrap();
        let close = sit(&tune, &backing, LineUp::LoopsFrom { start: 0, length: 2 * bar }, &Sitting::SHIPPED).unwrap();
        for (a, b) in close.by_move.iter().zip(&first.by_move) {
            assert!((a - b).abs() < 1e-9, "{close:?} {first:?}");
        }
    }

    #[test]
    fn chords_guessed_from_their_slots_find_the_tune_and_its_move() {
        let how = Sitting::SHIPPED;
        let written = Outline::guessed(&slots(&chords(0)), &how).sits(&slots(&tune(0))).unwrap();
        assert_eq!(written.best().0, 0, "{written:?}");
        // The chords saved a minor third up: the tune fits them moved up a minor third.
        let third_up = Outline::guessed(&slots(&chords(3)), &how).sits(&slots(&tune(0))).unwrap();
        assert_eq!(third_up.best().0, 3);
        assert!((third_up.best().1 - written.best().1).abs() < 1e-9);
        // A tune climbing by half steps sits far below the one written for them, whatever the move.
        let climbing: Vec<Note> = tune(0).into_iter().zip(60u8..).map(|(n, key)| Note { key, ..n }).collect();
        let off = Outline::guessed(&slots(&chords(0)), &how).sits(&slots(&climbing)).unwrap();
        assert!(off.best().1 < written.best().1 - 0.3, "{off:?} {written:?}");
    }

    #[test]
    fn nothing_to_look_at_gives_no_answer() {
        let how = Sitting::SHIPPED;
        let outline = backing(&chords(0)).outline(SLOT, &how);
        assert!(outline.sits(&[]).is_none());
        assert!(outline.sits(&[[0; 12]; 8]).is_none());
        assert!(Outline::guessed(&[], &how).sits(&slots(&tune(0))).is_none());
        // Chords with no sound anywhere: the tune is never heard over them.
        assert!(Outline::guessed(&[[0; 12]; 8], &how).sits(&slots(&tune(0))).is_none());
        // Far more slots than are ever looked at together, and lengths that never meet: neither hangs.
        let long = vec![[SLOT_FULL; 12]; 5000];
        assert!(Outline::guessed(&long, &how).sits(&long).is_some());
        let (seven, eleven) = (vec![[SLOT_FULL; 12]; 7], vec![[SLOT_FULL; 12]; 11]);
        assert!(Outline::guessed(&seven, &how).sits(&eleven).is_some());
        // A damaged file's times.
        let wild = [note(i64::MIN, i64::MAX, 60), note(i64::MAX - 5, i64::MAX, 64), note(0, -3, 67)];
        assert!(slots(&wild).len() <= MOST_SLOTS);
        assert!(slots_over(&wild, SLOT, i64::MIN, i64::MAX).len() <= MOST_SLOTS);
        assert!(slots_over(&wild, SLOT, 0, -5).is_empty());
    }
}
