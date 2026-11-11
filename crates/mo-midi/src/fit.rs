//! Fitting a part from the library to the song being worked on: a tune over the song's chords, or chords under the
//! song's tune. `mo-harmony` does the judging ([`mo_harmony::sit`]); this says what is fitted to what, and how the two
//! are lined up.
//!
//! What's fitted to is a [`Target`], made once: the picked parts of a song over some of its bars
//! ([`Target::of_song`], from [`crate::playing`]), or any chords or tune ([`Target::chords`], [`Target::tune`]: a part
//! of the library can be the target too). Each part tried against it is then quick ([`Target::fit`]). Measured in a
//! release build on the tests' 8-bar parts: 0.16 ms to make a target from a song's chords and bass, 7 microseconds
//! for a tune over chords, 0.23 ms for chords under a tune (their own chords are read first). The answer says how the
//! found part sits moved up by each of 0 to 11 semitones, so a part saved in another key comes with the move that
//! makes it fit.
//!
//! How the two are lined up: the bars picked from a song go round as picked, every one of them, a bar the picked
//! parts rest in too (the first or the last as well), so the part found is heard as it will sound dropped on the
//! first picked bar. The found part is a loop from the library, and how long that loop is isn't saved with it: it
//! goes round from the bar its first note is in to the bar its last note ends in, so a four-bar loop that rests in
//! its last bar is heard as three bars going round.
//!
//! A song that isn't in 4/4 is fitted to as it is, but the library's parts are all read as 4/4 (the MIDI page's
//! rule), so there the answer says less.
//!
//! Not here yet: trying every part of the library (a quick first pass over what the catalogue holds in memory, then
//! this close look at the best of them), and sending the part found into the project.

use std::ops::Range;

use mo_formats::NoteEvent;
use mo_harmony::{Backing, ChordContext, Grid, LineUp, Note, Part, Sits, Sitting, Slot, Span, sit};

use crate::PPQ;
use crate::chart;
use crate::keys::{heard_as, heard_notes};
use crate::playing::Song;
use crate::roles::Role;

/// How long a first look's slot is ([`mo_harmony::Slot`]), in the index's ticks: two quarter notes, half a bar of
/// 4/4, whatever the song's time signature.
pub const SLOT: i64 = 2 * PPQ;

/// What made the slots the index keeps of each set of notes ([`slots_of`]). Raised when what they hold changes, so
/// the index makes them again from the notes it keeps (rule 7).
pub const SLOTS_VERSION: i64 = 1;

/// A library part's slots, for a search's first look: how long it plays each pitch class in every [`SLOT`], from the
/// bar its first note is in to the bar its last note ends in (bars of 4/4, as every part of the library is read), its
/// first 32 bars at the most. The same stretch the close look hears go round ([`Target::fit`]).
pub fn slots_of(notes: &[NoteEvent]) -> Vec<Slot> {
    mo_harmony::slots_of(&heard_notes(notes), Grid::four_four(PPQ as u32), SLOT)
}

/// What's looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wants {
    /// A tune (or a bassline, or an arp: anything played over chords) to go over the target's chords.
    Tune,
    /// Chords to go under the target's tune.
    Chords,
}

/// What a part from the library is fitted to.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    given: Given,
}

#[derive(Clone, Debug, PartialEq)]
enum Given {
    /// Chords, read once.
    Chords(Backing),
    /// A tune, and how it goes round.
    Tune { notes: Vec<Note>, line_up: LineUp },
}

impl Target {
    /// Chords to find a tune for: `parts` are what plays together (the chords, the bass), each with its kind, on
    /// `grid`. `span`: the ticks they're a stretch of, from a bar line to a bar line, when they're cut from a longer
    /// piece (bars picked from a song): the first chord needn't come after the last, a bar the parts rest in is still
    /// one of the bars that go round, and what they play outside it is left out. Without one they're a whole loop,
    /// from the bar of its first note to the bar its last note ends in. None for drums alone, a span with no length,
    /// or when no chord can be read in them.
    pub fn chords(parts: &[(Role, &[NoteEvent])], grid: Grid, span: Option<Range<i64>>) -> Option<Target> {
        let heard: Vec<(Role, Vec<Note>)> = parts
            .iter()
            .filter(|(role, _)| *role != Role::Drums)
            .map(|(role, notes)| (*role, heard_notes(notes)))
            .collect();
        let parts: Vec<Part> = heard
            .iter()
            .filter(|(_, notes)| !notes.is_empty())
            .map(|(role, notes)| Part { role: heard_as(*role), notes })
            .collect();
        if parts.is_empty() {
            return None;
        }
        let backing = chart::with_reader(|reader| match span {
            Some(span) => {
                let length = span.end.checked_sub(span.start).filter(|l| *l > 0)?;
                let reading = reader.read(&parts, grid, &ChordContext::new(Span::Window))?;
                Some(Backing::over(&reading, &parts, span.start, length))
            }
            None => Backing::read(reader, &parts, grid, Span::Loop),
        })?;
        Some(Target { given: Given::Chords(backing) })
    }

    /// A tune to find chords for: its notes, with tick 0 on one of its bar lines. `span`: the ticks it goes round in,
    /// from a bar line to a bar line (bars picked from a song: a bar the tune rests in is still one of them; its
    /// notes lie within it). Without one it goes round from the bar of its first note to the bar its last note ends
    /// in. None without notes, or for a span with no length.
    pub fn tune(notes: &[NoteEvent], span: Option<Range<i64>>) -> Option<Target> {
        let line_up = match span {
            Some(span) => {
                let length = span.end.checked_sub(span.start).filter(|l| *l > 0)?;
                LineUp::LoopsFrom { start: span.start, length }
            }
            None => LineUp::Loops { bar_line: 0 },
        };
        let notes = heard_notes(notes);
        (!notes.is_empty()).then_some(Target { given: Given::Tune { notes, line_up } })
    }

    /// The picked parts of a song (places in [`Song::parts`]) over `bars` of it, counted from 0: a tune to find chords
    /// for when every one of them is a melody, else chords to find a tune for. Drums are left out; a sampler's notes
    /// ([`crate::playing::Playing::sample`]) are heard as written when it's picked. Every picked bar counts, one the
    /// parts rest in too. None when the picked parts play nothing pitched in those bars.
    pub fn of_song(song: &Song, parts: &[usize], bars: &Range<i64>) -> Option<Target> {
        // Each part once, in the song's order, however it was picked.
        let mut places = parts.to_vec();
        places.sort_unstable();
        places.dedup();
        let picked: Vec<(Role, Vec<NoteEvent>)> = places
            .into_iter()
            .filter_map(|at| {
                let part = song.parts.get(at).filter(|p| !p.drums && p.role != Role::Drums)?;
                let notes = song.stretch(at, bars);
                (!notes.is_empty()).then_some((part.role, notes))
            })
            .collect();
        if picked.is_empty() {
            return None;
        }
        // The picked bars in ticks, the first of them at 0 as `Song::stretch` gives their notes.
        let span = Some(0..bars.end.saturating_sub(bars.start).saturating_mul(song.bar()));
        if picked.iter().all(|(role, _)| *role == Role::Melody) {
            let mut tune: Vec<NoteEvent> = picked.into_iter().flat_map(|(_, notes)| notes).collect();
            tune.sort_by_key(|n| (n.start, n.key));
            return Target::tune(&tune, span);
        }
        let parts: Vec<(Role, &[NoteEvent])> = picked.iter().map(|(role, notes)| (*role, notes.as_slice())).collect();
        Target::chords(&parts, song.grid(), span)
    }

    /// What kind of part goes with it.
    pub fn wants(&self) -> Wants {
        match self.given {
            Given::Chords(_) => Wants::Tune,
            Given::Tune { .. } => Wants::Chords,
        }
    }

    /// Whether a part of this kind is worth trying against it: anything but drums over chords; under a tune, a part
    /// that plays chords.
    pub fn tries(&self, role: Role) -> bool {
        match self.given {
            Given::Chords(_) => role != Role::Drums,
            Given::Tune { .. } => matches!(role, Role::Chords | Role::Arp | Role::Other),
        }
    }

    /// How a part from the library (its notes at [`crate::PPQ`] from its own bar 1, its kind, and whether it was `cut`
    /// from a longer stretch) sits with the target, with the part found moved up by each of 0 to 11 semitones:
    /// [`Sits::best`] gives the move that fits best and how it sits there, [`Sits::moved`] how it sits as it is (0)
    /// or moved any other way. None for drums or no notes, when the part found hardly plays where the target has
    /// sound, or, under a tune, when no chords can be read in it.
    pub fn fit(&self, notes: &[NoteEvent], role: Role, cut: bool) -> Option<Sits> {
        if role == Role::Drums {
            return None;
        }
        match &self.given {
            // The part found goes round from its own first bar.
            Given::Chords(backing) => {
                sit(&heard_notes(notes), backing, LineUp::Loops { bar_line: 0 }, &Sitting::SHIPPED)
            }
            Given::Tune { notes: tune, line_up } => {
                let backing = chart::backing(notes, role, cut)?;
                let tune_moved = sit(tune, &backing, *line_up, &Sitting::SHIPPED)?;
                // The chords moved up by a number of semitones under the tune sound as the tune moved down by as
                // many over the chords.
                let mut by_move = [0.0; 12];
                for (up, sits) in (0i32..).zip(&mut by_move) {
                    *sits = tune_moved.moved(-up);
                }
                Some(Sits { by_move, heard: tune_moved.heard })
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::{BAR, PPQ, playing};
    use mo_formats::NoteAttribute;
    use mo_formats::arrangement::{Arrangement, LaneKind, PreviewClip, PreviewLane, PreviewNote};

    fn note(start: i64, length: i64, key: u8) -> NoteEvent {
        NoteEvent {
            start,
            length,
            key,
            velocity: 40_000,
            muted: false,
            step: false,
            attribute: NoteAttribute::default(),
        }
    }

    /// Am7, Dm7, G7, Cmaj7, a bar each, moved up by `up` semitones.
    fn chords(up: u8) -> Vec<NoteEvent> {
        [[57, 60, 64, 67], [50, 60, 65, 69], [55, 59, 62, 65], [48, 59, 64, 67]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, bar)| keys.iter().map(move |&k| note(bar * BAR, BAR, k + up)))
            .collect()
    }

    /// Their roots, low, a bar each.
    fn bass() -> Vec<NoteEvent> {
        [33, 38, 31, 36].iter().zip(0i64..).map(|(&k, bar)| note(bar * BAR, BAR, k)).collect()
    }

    /// A tune on the chords' thirds, sevenths and roots, two notes a beat apart then a held one each bar, moved up by
    /// `up` semitones.
    fn tune(up: u8) -> Vec<NoteEvent> {
        [[72, 76, 79], [77, 72, 74], [71, 77, 74], [76, 71, 72]]
            .iter()
            .zip(0i64..)
            .flat_map(|(keys, bar)| {
                [
                    note(bar * BAR, PPQ, keys[0] + up),
                    note(bar * BAR + PPQ, PPQ, keys[1] + up),
                    note(bar * BAR + 2 * PPQ, 2 * PPQ, keys[2] + up),
                ]
            })
            .collect()
    }

    /// The same rhythm climbing by half steps: whichever way it's moved, most of it is off the chords.
    fn clashing() -> Vec<NoteEvent> {
        tune(0).into_iter().zip(60u8..).map(|(n, key)| NoteEvent { key, ..n }).collect()
    }

    fn lane(name: &str, at_bar: i64, notes: &[NoteEvent]) -> PreviewLane {
        let beats = |ticks: i64| ticks as f64 / PPQ as f64;
        let notes: Vec<PreviewNote> = notes
            .iter()
            .map(|n| PreviewNote {
                start: beats(n.start),
                length: beats(n.length),
                key: n.key,
                velocity: n.velocity,
                ..PreviewNote::default()
            })
            .collect();
        let clip =
            PreviewClip { name: name.into(), start: beats(at_bar * BAR), length: 16.0, notes, ..Default::default() };
        PreviewLane { clips: vec![clip], ..PreviewLane::new(name, LaneKind::Instrument) }
    }

    /// A song whose chords and bass come in at bar 9 and whose lead plays the tune over them from bar 13 (the chords
    /// go round twice).
    fn song() -> playing::Song {
        let twice: Vec<NoteEvent> = chords(0)
            .into_iter()
            .chain(chords(0).into_iter().map(|n| NoteEvent { start: n.start + 4 * BAR, ..n }))
            .collect();
        let mut keys = lane("Keys", 8, &twice);
        keys.clips[0].length = 32.0;
        playing::of(&Arrangement {
            view: "Test",
            tempo: 120.0,
            beats_per_bar: 4,
            lanes: vec![keys, lane("Sub", 8, &bass()), lane("Lead", 12, &tune(0)), lane("FPC", 8, &bass())],
            markers: Vec::new(),
            notes: Vec::new(),
        })
    }

    #[test]
    fn a_tune_in_another_key_is_found_with_the_move_that_fits_it() {
        let song = song();
        assert_eq!(
            song.parts.iter().map(|p| p.role).collect::<Vec<_>>(),
            [Role::Chords, Role::Bass, Role::Melody, Role::Drums]
        );
        assert_eq!((song.harmony_parts(), song.melody_parts()), (vec![0, 1], vec![2]));
        // The song's chords and bass over bars 9 to 12.
        let target = Target::of_song(&song, &song.harmony_parts(), &(8..12)).unwrap();
        assert_eq!(target.wants(), Wants::Tune);
        // The tune as written sits well as it is; saved a fourth up, it's found with the move back down.
        let written = target.fit(&tune(0), Role::Melody, false).unwrap();
        assert_eq!(written.best().0, 0);
        assert!(written.best().1 > 0.8, "{written:?}");
        let fourth_up = target.fit(&tune(5), Role::Melody, false).unwrap();
        assert_eq!(fourth_up.best().0, -5);
        assert!((fourth_up.best().1 - written.best().1).abs() < 1e-9);
        assert!(fourth_up.moved(0) < written.moved(0) - 0.3, "{fourth_up:?}");
        // A tune a half step off the chords sits far below it, whatever the move.
        let off = target.fit(&clashing(), Role::Melody, false).unwrap();
        assert!(off.best().1 < written.best().1 - 0.3, "{off:?}");
        // Drums are never fitted, and nor is nothing.
        assert!(target.fit(&tune(0), Role::Drums, false).is_none());
        assert!(target.fit(&[], Role::Melody, false).is_none());
        assert!(target.tries(Role::Bass) && target.tries(Role::Arp) && !target.tries(Role::Drums));
    }

    #[test]
    fn chords_in_another_key_are_found_for_the_songs_tune() {
        let song = song();
        // The lead alone, where it plays: a tune to find chords for.
        let bars = song.busiest_bars(&[2], 8).unwrap();
        assert_eq!(bars, 12..16);
        let target = Target::of_song(&song, &[2], &bars).unwrap();
        assert_eq!(target.wants(), Wants::Chords);
        let written = target.fit(&chords(0), Role::Chords, false).unwrap();
        assert_eq!(written.best().0, 0);
        assert!(written.best().1 > 0.8, "{written:?}");
        // The same chords saved a minor third up: found with the move back down, sitting the same there.
        let third_up = target.fit(&chords(3), Role::Chords, false).unwrap();
        assert_eq!(third_up.best().0, -3);
        assert!((third_up.best().1 - written.best().1).abs() < 1e-9);
        // Both ways round agree: the song's chords with this tune, or the song's tune with these chords.
        let other_way = Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), None)
            .unwrap()
            .fit(&tune(0), Role::Melody, false)
            .unwrap();
        assert!((other_way.moved(0) - written.moved(0)).abs() < 1e-9);
        // Under a tune only parts that play chords are tried.
        assert!(target.tries(Role::Chords) && target.tries(Role::Arp));
        assert!(!target.tries(Role::Melody) && !target.tries(Role::Bass) && !target.tries(Role::Drums));
        assert!(target.fit(&chords(0), Role::Drums, false).is_none());
    }

    #[test]
    fn the_bars_picked_are_what_is_fitted_to() {
        // Four bars of the chords, then the same four a tritone up: the tune fits the first four as it is, and the
        // second four moved a tritone.
        let both: Vec<NoteEvent> = chords(0)
            .into_iter()
            .chain(chords(6).into_iter().map(|n| NoteEvent { start: n.start + 4 * BAR, ..n }))
            .collect();
        let mut keys = lane("Keys", 0, &both);
        keys.clips[0].length = 32.0;
        let song = playing::of(&Arrangement {
            view: "Test",
            tempo: 120.0,
            beats_per_bar: 4,
            lanes: vec![keys],
            markers: Vec::new(),
            notes: Vec::new(),
        });
        let first = Target::of_song(&song, &[0], &(0..4)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        let second = Target::of_song(&song, &[0], &(4..8)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        assert_eq!(first.best().0, 0);
        assert_eq!(second.best().0, -6);
        assert!((first.best().1 - second.best().1).abs() < 1e-9);
    }

    #[test]
    fn a_bar_the_picked_parts_rest_in_is_still_one_of_the_bars() {
        let grid = Grid::four_four(PPQ as u32);
        let three_bars =
            |notes: Vec<NoteEvent>| -> Vec<NoteEvent> { notes.into_iter().filter(|n| n.start < 3 * BAR).collect() };
        let song_of = |lanes: Vec<PreviewLane>| {
            playing::of(&Arrangement {
                view: "Test",
                tempo: 120.0,
                beats_per_bar: 4,
                lanes,
                markers: Vec::new(),
                notes: Vec::new(),
            })
        };
        // A lead that rests in the last of the four bars picked: the four bars of chords written for it still line
        // up with it bar for bar.
        let lead = song_of(vec![lane("Lead", 12, &three_bars(tune(0)))]);
        let target = Target::of_song(&lead, &[0], &(12..16)).unwrap();
        assert_eq!(target.wants(), Wants::Chords);
        let rested = target.fit(&chords(0), Role::Chords, false).unwrap();
        assert_eq!(rested.best().0, 0);
        // Heard as three bars going round (no bars given), the tune slips against the chords and sits lower.
        let slips = Target::tune(&three_bars(tune(0)), None).unwrap().fit(&chords(0), Role::Chords, false).unwrap();
        assert!(rested.moved(0) > slips.moved(0) + 0.1, "{rested:?} {slips:?}");

        // Chords that rest in the last picked bar (Am7, Dm7, G7, a bar of nothing): the same for a four-bar tune,
        // whose last bar then plays over the rest.
        let keys = song_of(vec![lane("Keys", 8, &three_bars(chords(0)))]);
        let rested = Target::of_song(&keys, &[0], &(8..12)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        assert_eq!(rested.best().0, 0);
        assert!(rested.heard < 0.9, "{rested:?}");
        let slips = Target::chords(&[(Role::Chords, &three_bars(chords(0)))], grid, None)
            .unwrap()
            .fit(&tune(0), Role::Melody, false)
            .unwrap();
        assert!((slips.heard - 1.0).abs() < 1e-9);
        assert!(rested.moved(0) > slips.moved(0) + 0.1, "{rested:?} {slips:?}");

        // Picked from the bar before the chords come in, the tune found is heard as it will sound dropped on that
        // bar: a bar early for these chords.
        let song = song();
        let on_time = Target::of_song(&song, &[0, 1], &(8..12)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        let early = Target::of_song(&song, &[0, 1], &(7..11)).unwrap().fit(&tune(0), Role::Melody, false).unwrap();
        assert!(early.moved(0) < on_time.moved(0) - 0.1, "{early:?} {on_time:?}");
    }

    #[test]
    fn nothing_to_fit_to_gives_no_target() {
        let song = song();
        // No part picked, a part that isn't there, drums alone, or bars where the picked parts are silent.
        assert!(Target::of_song(&song, &[], &(8..12)).is_none());
        assert!(Target::of_song(&song, &[9], &(8..12)).is_none());
        assert!(Target::of_song(&song, &[3], &(8..12)).is_none());
        assert!(Target::of_song(&song, &[0, 1], &(0..8)).is_none());
        // Drums picked beside the chords are left out, not heard as notes.
        let with_drums = Target::of_song(&song, &[0, 3], &(8..12)).unwrap();
        assert_eq!(with_drums, Target::of_song(&song, &[0], &(8..12)).unwrap());
        assert!(Target::tune(&[], None).is_none());
        assert!(Target::chords(&[], Grid::four_four(PPQ as u32), None).is_none());
        assert!(Target::chords(&[(Role::Drums, &bass())], Grid::four_four(PPQ as u32), None).is_none());
        // A span with no length, or the wrong way round, is nothing to fit to.
        #[allow(clippy::reversed_empty_ranges)]
        let backwards = Some(4 * BAR..0);
        assert!(Target::tune(&tune(0), Some(0..0)).is_none());
        assert!(Target::tune(&tune(0), backwards.clone()).is_none());
        assert!(Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), Some(0..0)).is_none());
        assert!(Target::chords(&[(Role::Chords, &chords(0))], Grid::four_four(PPQ as u32), backwards).is_none());
    }
}
