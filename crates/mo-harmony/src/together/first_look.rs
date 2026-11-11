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
//! eighth in 6/8, so two of its beats wouldn't be the same stretch of time in every song.) A loop that doesn't end
//! on a slot's line (a bar of 6/8 is a slot and a half) is kept going round until it does, so slots go round just as
//! the loop does.
//!
//! No more than [`MOST_SLOTS`] are kept of anything: a part longer than 32 bars of 4/4 is looked at by its first 32
//! alone, and the close look hears the rest.
//!
//! How much it may leave out was measured (`theorybench pairs --quick`; `docs/harmony/together-experiments.md` has
//! the numbers): for a tune over chords, keeping the best 2 parts in 100 keeps 99 in 100 of the close look's best
//! ten; for chords under a tune, guessed, it takes the best 10 in 100 to keep 98. What a library held before, how
//! long a part plays each pitch class over all of it with no timing, doesn't do: the best 5 in 100 by that keep 7
//! in 100 of the close look's best ten tunes and 16 in 100 of its best ten chord parts.

use super::{
    ABOVE, ALIKE, BESIDE, Backing, COLOUR, FIFTH, LEAST_HEARD, MOST_BARS_ROUND, NEAR_PLAYED, NO_CHORD, Near, OUTSIDE,
    RARE, ROOT, Sits, Sitting, THIRD, alike_by_move, gcd, near_of, own_bars, strongest, times_in,
};
use crate::notes::{Grid, Note};
use crate::pitch::{PerPitch, PitchClass};

/// How long a part plays each pitch class (from C up) in one slot of time, two quarter notes as a rule:
/// [`SLOT_FULL`] for all of it. The same note in two octaves counts twice, up to 255.
pub type Slot = [u8; 12];

/// A pitch class played all through a [`Slot`].
pub const SLOT_FULL: u8 = 64;

/// What [`slots_of`] and [`slots_over`] make of a part's notes. Raised when the same notes would give other slots
/// (how much of a slot a note fills, how a loop goes round, how many are kept), so slots that are kept somewhere
/// are made again (rule 7). What's then done with slots ([`Outline`]) isn't part of it.
pub const SLOTS_VERSION: u32 = 1;

/// The most slots kept of a part, or of chords: 32 bars of 4/4 in half bars, which is as long as the close look
/// plays two loops round for.
pub const MOST_SLOTS: usize = 2 * MOST_BARS_ROUND as usize;

/// How many slots a loop `length` ticks long is kept for: until it ends on a slot's line, going round again when
/// once through ends inside a slot. [`MOST_SLOTS`] at the most.
fn slots_round(length: i64, slot: i64) -> usize {
    usize::try_from(length / gcd(length, slot)).unwrap_or(MOST_SLOTS).min(MOST_SLOTS)
}

/// A part's slots, each `slot` ticks long, from the bar its first note is in to the bar its last note ends in, as
/// the close look hears a loop go round ([`LineUp::Loops`](super::LineUp::Loops)). Its first [`MOST_SLOTS`] at the
/// most; none for no notes.
pub fn slots_of(notes: &[Note], grid: Grid, slot: i64) -> Vec<Slot> {
    match own_bars(notes, grid.bar(), grid.bar_line) {
        Some((from, length)) => slots_over(notes, slot, from, length),
        None => Vec::new(),
    }
}

/// A part's slots, each `slot` ticks long, over `length` ticks from `from`, a bar line (bars picked from a song: one
/// the part rests in is still a slot or two). What it plays outside them is left out. Where the stretch ends inside
/// a slot it goes round again until it ends on a slot's line. The first [`MOST_SLOTS`] at the most; none for no
/// length.
pub fn slots_over(notes: &[Note], slot: i64, from: i64, length: i64) -> Vec<Slot> {
    if length <= 0 {
        return Vec::new();
    }
    let slot = slot.max(1);
    times_in(notes.iter(), from, length, slot, slots_round(length, slot))
        .iter()
        .map(|time| {
            time.0.map(|t| {
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
    /// For each slot, from C up; none where nothing is heard. The chords going round until they end on a slot's
    /// line, so it may be more than once through.
    worth: Vec<Option<[f64; 12]>>,
    /// How many of the slots are once through the chords.
    once: usize,
    /// The most slots a tune and the chords are played round for: past it, once through the longer. As many as the
    /// close look's [`MOST_BARS_ROUND`] bars of the chords' own.
    most: usize,
    /// How much of the chords' sounding time each pitch class takes, the most-played 1.
    scale: PerPitch<f64>,
    /// What being alike is worth ([`Sitting`]'s `alike`).
    alike: f64,
}

/// A note's worth with what sounds near it added: the close look's `near_played`, and its rub over each note
/// played 1 to 11 semitones under it.
fn with_near(worth: f64, near: &Near, worth_of: impl Fn(usize) -> f64) -> f64 {
    let mut worth = worth;
    if near.played {
        worth += worth_of(NEAR_PLAYED);
    }
    for (d, strength) in near.above.iter().enumerate().filter(|(_, s)| **s > 0.0) {
        worth += strength * worth_of(ABOVE + d);
    }
    worth
}

impl Backing {
    /// The backing as a first look hears it, in slots of `slot` ticks and with these numbers: over each slot, what
    /// the close look gives a note of each pitch class there, averaged over the slot. A tune's slots are then tried
    /// against it in about a microsecond ([`Outline::sits`]).
    pub fn outline(&self, slot: i64, how: &Sitting) -> Outline {
        let worths = how.worths();
        let worth_of = |place: usize| worths.get(place).copied().unwrap_or(0.0);
        let (slot, half, length) = (slot.max(1), self.half(), self.length.max(1));
        let count = slots_round(length, slot);
        let mut worth = Vec::with_capacity(count);
        for k in 0..count as i64 {
            let to = (k + 1).saturating_mul(slot);
            let mut sums = [0.0f64; 12];
            let mut heard = 0i64;
            // Each chord and each half bar in the slot in turn, the backing going round under it, as the close look
            // walks them under a note.
            let mut at = k.saturating_mul(slot);
            while at < to {
                let within = at.rem_euclid(length);
                let lap = at - within;
                let s = self.stretches.partition_point(|s| s.end <= within);
                let stretch = self.stretches.get(s).filter(|s| s.start <= within);
                let until = match stretch {
                    Some(s) => s.end,
                    None => self.stretches.get(s).map_or(length, |s| s.start),
                }
                .min(length)
                .min((within / half + 1).saturating_mul(half))
                .saturating_add(lap)
                .min(to)
                .max(at + 1);
                let near = usize::try_from(within / half).ok().and_then(|h| self.halves.get(h).copied().flatten());
                let time = until - at;
                at = until;
                // Where there's neither a chord nor sound, a note isn't heard.
                if stretch.is_none() && near.is_none() {
                    continue;
                }
                heard += time;
                for (pc, sum) in PitchClass::ALL.into_iter().zip(&mut sums) {
                    let kind = worth_of(stretch.map_or(NO_CHORD, |s| s.kinds[pc]));
                    let worth = near.map_or(kind, |near| with_near(kind, &near[pc], worth_of));
                    *sum += time as f64 * worth;
                }
            }
            worth.push((heard > 0).then(|| sums.map(|sum| sum / heard as f64)));
        }
        let slots_in = |ticks: i64| usize::try_from(ticks / slot).unwrap_or(usize::MAX);
        Outline {
            worth,
            once: slots_in(length.saturating_add(slot - 1)).min(count),
            most: slots_in(MOST_BARS_ROUND.saturating_mul(self.grid.bar())).max(1),
            scale: self.scale,
            alike: worth_of(ALIKE),
        }
    }
}

impl Outline {
    /// Chords guessed from a chord part's slots alone, with no chord read in them: a note played in a slot is taken
    /// for a note of the chord there, one that isn't for a note beside it, rubbing the ones that are as the close
    /// look counts it. For trying chord parts under a tune without reading each one's chords first. The slots are a
    /// part's read in 4/4, half a bar each, as a library keeps them.
    pub fn guessed(slots: &[Slot], how: &Sitting) -> Outline {
        let worths = how.worths();
        let worth_of = |place: usize| worths.get(place).copied().unwrap_or(0.0);
        let slots = slots.get(..MOST_SLOTS).unwrap_or(slots);
        // The same note in two octaves is one note of the chord: no pitch class counts for more than the whole
        // slot. (Measured: without this the guess keeps 4 in 100 fewer of the close look's best.)
        let heard = |slot: &Slot| PerPitch(slot.map(|t| i64::from(t.min(SLOT_FULL))));
        let mut whole = PerPitch([0i64; 12]);
        for slot in slots {
            for (pc, time) in heard(slot).iter() {
                whole[pc] = whole[pc].saturating_add(time);
            }
        }
        let scale = strongest(&whole).unwrap_or_default();
        // A played note could be any note of the chord.
        let chord_note = (worth_of(ROOT) + worth_of(THIRD) + worth_of(FIFTH) + worth_of(COLOUR)) / 4.0;
        let worth = slots
            .iter()
            .map(|slot| {
                let near = near_of(&strongest(&heard(slot))?);
                let mut out = [0.0f64; 12];
                for (pc, cell) in PitchClass::ALL.into_iter().zip(&mut out) {
                    let kind = if near[pc].played {
                        chord_note
                    } else {
                        worth_of(if scale[pc] >= RARE { BESIDE } else { OUTSIDE })
                    };
                    *cell = with_near(kind, &near[pc], worth_of);
                }
                Some(out)
            })
            .collect();
        Outline { worth, once: slots.len(), most: MOST_SLOTS, scale, alike: worth_of(ALIKE) }
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
        // Until they meet again, or once through the longer when that's past the most two loops are played round.
        let shared = usize::try_from(gcd(mine as i64, theirs as i64)).unwrap_or(1).max(1);
        let together = mine / shared * theirs;
        let round = if together <= self.most { together } else { mine.max(self.once) };
        let mut sums = [0.0f64; 12];
        let mut tune_time = PerPitch([0.0f64; 12]);
        let (mut heard, mut all) = (0.0f64, 0.0f64);
        for k in 0..round {
            let (Some(slot), Some(worth)) = (tune.get(k % mine), self.worth.get(k % theirs)) else { continue };
            let time: u32 = slot.iter().map(|t| u32::from(*t)).sum();
            all += f64::from(time);
            let Some(worth) = worth else { continue };
            heard += f64::from(time);
            for (pc, time) in PitchClass::ALL.into_iter().zip(slot).filter(|(_, t)| **t > 0) {
                let time = f64::from(*time);
                tune_time[pc] += time;
                for (s, sum) in (0i32..).zip(sums.iter_mut()) {
                    *sum += time * worth.get(pc.up(s).index()).copied().unwrap_or(0.0);
                }
            }
        }
        if all <= 0.0 || heard < LEAST_HEARD * all {
            return None;
        }
        // The tune as a whole beside the chords as a whole.
        let mut by_move = [0.0f64; 12];
        for ((sits, sum), alike) in by_move.iter_mut().zip(&sums).zip(alike_by_move(&tune_time, &self.scale)) {
            *sits = sum / heard + self.alike * alike;
        }
        Some(Sits { by_move, heard: (heard / all).min(1.0) })
    }
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

    /// Bars of 6/8 at 48 ticks an eighth, a chord a bar. A bar is 288 ticks: a slot and a half.
    fn six_eight(bars: &[[u8; 4]]) -> Backing {
        let grid = Grid { ticks_per_beat: 48, beats_per_bar: 6, bar_line: 0 };
        let chords: Vec<Note> = bars
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, at)| keys.iter().map(move |&k| note(at * grid.bar(), grid.bar(), k)))
            .collect();
        let parts = [Part { role: Role::Harmony, notes: &chords }];
        Backing::read(&mut ChordReader::new(), &parts, grid, Span::Loop).unwrap()
    }

    const A_MINOR: [u8; 4] = [45, 57, 60, 64];
    const G_MAJOR: [u8; 4] = [43, 55, 59, 62];

    /// A tune from the library, read as 4/4 as every part there is: a note a slot.
    fn a_note_a_slot(keys: &[u8]) -> Vec<Note> {
        keys.iter().zip(0i64..).map(|(&k, at)| note(at * SLOT, SLOT, k)).collect()
    }

    /// The first look of a tune of whole slots over a backing, beside the close look of the same two loops.
    fn both_looks(tune: &[Note], backing: &Backing) -> (Sits, Sits) {
        let length = tune.len() as i64 * SLOT;
        let first = backing.outline(SLOT, &Sitting::SHIPPED).sits(&slots_over(tune, SLOT, 0, length)).unwrap();
        let close = sit(tune, backing, LineUp::LoopsFrom { start: 0, length }, &Sitting::SHIPPED).unwrap();
        (first, close)
    }

    fn assert_same(first: &Sits, close: &Sits) {
        for (a, b) in close.by_move.iter().zip(&first.by_move) {
            assert!((a - b).abs() < 1e-9, "{close:?} {first:?}");
        }
        assert!((first.heard - close.heard).abs() < 1e-9);
    }

    #[test]
    fn a_song_in_six_eight_is_looked_at_in_the_same_slots_as_a_part_read_in_four_four() {
        // Two bars are three slots, the middle one over the bar line.
        let (first, close) = both_looks(&a_note_a_slot(&[72, 71, 74]), &six_eight(&[A_MINOR, G_MAJOR]));
        assert_same(&first, &close);
    }

    #[test]
    fn a_loop_that_ends_inside_a_slot_goes_round_until_it_ends_on_a_slots_line() {
        // One bar of 6/8 is a slot and a half: twice round is three slots, and the second slot hears the end of the
        // bar and its start again.
        let one_bar = six_eight(&[A_MINOR]);
        assert_eq!(one_bar.outline(SLOT, &Sitting::SHIPPED).worth.len(), 3);
        let (first, close) = both_looks(&a_note_a_slot(&[72, 76, 69]), &one_bar);
        assert_same(&first, &close);
        // Three bars are four slots and a half: nine slots, twice round.
        let three_bars = six_eight(&[A_MINOR, G_MAJOR, A_MINOR]);
        assert_eq!(three_bars.outline(SLOT, &Sitting::SHIPPED).worth.len(), 9);
        let (first, close) = both_looks(&a_note_a_slot(&[72, 71, 74, 76, 79, 77, 72, 67, 69]), &three_bars);
        assert_same(&first, &close);
        // A part's slots the same way: an eighth note at the start of a bar of 6/8 is heard in the first slot, and
        // again in the second, where the bar comes round.
        let bar = slots_over(&[note(0, 48, 60)], SLOT, 0, 288);
        assert_eq!(bar.len(), 3);
        assert_eq!((bar[0][0], bar[1][0], bar[2][0]), (SLOT_FULL / 4, SLOT_FULL / 4, 0));
    }

    #[test]
    fn two_loops_are_played_round_for_as_long_as_the_close_look_plays_them() {
        // Two bars of 6/8 (three slots) and a tune of 20 slots meet again after 60 slots: more than 32 bars of 6/8
        // (48 slots), so both looks play the tune once through.
        let keys: Vec<u8> = (0..20u8).map(|i| 60 + (i * 7) % 12).collect();
        let backing = six_eight(&[A_MINOR, G_MAJOR]);
        let (first, close) = both_looks(&a_note_a_slot(&keys), &backing);
        assert_same(&first, &close);
        // And a tune of 16 slots meets them after exactly 48: played all the way round.
        let (first, close) = both_looks(&a_note_a_slot(&keys[..16]), &backing);
        assert_same(&first, &close);
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
