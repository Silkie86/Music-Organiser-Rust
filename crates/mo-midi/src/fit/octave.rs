//! The octave a part found goes at beside a song.
//!
//! A search says how many semitones to move a part so its notes go with the song's ([`super::Found::sits`]). That
//! settles its key and leaves its octave open: a bassline saved two octaves up is in key all the same. This says the
//! octave ([`Registers::octaves_for`], which a search calls for each part it finds): the one that brings the middle
//! of the part ([`middle`]) nearest to where a part of its kind belongs beside the song ([`Registers::wanted`]).
//! That's where its kind usually plays ([`usual`]); for a bassline or chords, which would stand in for the song's
//! own, it's half-way from there to where the song's bass or chords play. Where they play in the song is where they
//! play in the bars fitted to, when they play there ([`Registers::by_bars`]).
//!
//! Measured on POP909-CL, 8 bars at a time (`theorybench octave`, `docs/harmony/together-experiments.md`): how often
//! a part taken out of its song is put back at the octave it was written at. No rule gets past about 3 in 4,
//! because the octave a tune is written at is partly taste: it's a first guess, and changing it by hand is the
//! page's next step. What the numbers do say:
//!
//! - The usual place alone does as well as anything. Where the song's other kinds of part play says next to
//!   nothing about this one (the C# app nudged the usual place by them, which loses 20 points and more), so they
//!   aren't asked.
//! - A second tune written to go with a song's first is less often at the first tune's height than at a tune's usual
//!   height: two lines keep out of each other's way. So a tune found isn't pulled to the song's tune at all
//!   (half-way was tried: never better, up to 2 points worse), and nor is an arp or a part of no kind.
//! - A bassline and chords are pulled half-way to the song's own. That one isn't measured (the songs have one
//!   accompaniment each): it's there so that a bassline found for a song with a low sub lands by the sub.
//! - Which middle is taken matters little; this one isn't thrown by a stray note (a key switch far below the part).

use std::ops::Range;

use mo_formats::NoteEvent;

use crate::PPQ;
use crate::playing::Song;
use crate::roles::Role;

/// The most octaves a part is moved, either way.
pub const MOST_OCTAVES: i32 = 3;

/// A note counts for at least this many beats and at most this many in a part's [`middle`]: a run of short notes
/// isn't outweighed by one held note, and a drone doesn't decide it all.
const LEAST_BEATS: f64 = 1.0;
const MOST_BEATS: f64 = 8.0;

/// Less than this share of a part's sounding time, at its low end or at its high end, is strays.
const STRAYS: f64 = 0.05;

/// Where a part of this kind usually plays: the middle of it ([`middle`]), as a key number (60 is middle C). None
/// for drums, whose notes pick pads. A tune's is measured (POP909-CL: 72 leaves the most tunes at their own
/// octave); an accompaniment with both hands measures 55 or 56 there, and chords alone sit a little higher. The
/// others are the C# app's.
pub fn usual(role: Role) -> Option<f64> {
    match role {
        Role::Bass => Some(40.0),
        Role::Chords => Some(58.0),
        Role::Arp => Some(67.0),
        Role::Melody => Some(72.0),
        Role::Other => Some(62.0),
        Role::Drums => None,
    }
}

/// How long each key sounds in some notes, a note's time held to [`LEAST_BEATS`] and [`MOST_BEATS`]; muted notes
/// left out.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sounding([f64; 128]);

impl Sounding {
    const NONE: Sounding = Sounding([0.0; 128]);

    fn add(&mut self, notes: &[NoteEvent]) {
        for n in notes.iter().filter(|n| !n.muted) {
            let beats = (n.length as f64 / PPQ as f64).clamp(LEAST_BEATS, MOST_BEATS);
            if let Some(time) = self.0.get_mut(usize::from(n.key)) {
                *time += beats;
            }
        }
    }

    /// The lowest and the highest key the part itself plays: the strays at each end aside, which are the keys from
    /// that end that sound for less than [`STRAYS`] of the time between them. The same rule from both ends, so a
    /// part turned upside down has the same edges turned. None when nothing sounds.
    fn edges(&self) -> Option<(u8, u8)> {
        let all: f64 = self.0.iter().sum();
        if all <= 0.0 {
            return None;
        }
        // Counting in from one end: the first key by which that share of the time has sounded.
        let edge = |keys: &mut dyn Iterator<Item = (u8, &f64)>| {
            let mut so_far = 0.0;
            keys.filter(|(_, time)| **time > 0.0).find_map(|(key, time)| {
                so_far += time;
                (so_far >= STRAYS * all).then_some(key)
            })
        };
        let keys = || (0u8..=NoteEvent::TOP_KEY).zip(&self.0);
        Some((edge(&mut keys())?, edge(&mut keys().rev())?))
    }

    /// The middle of the keys the part plays: half-way between its [`Sounding::edges`].
    fn middle(&self) -> Option<f64> {
        let (low, high) = self.edges()?;
        Some((f64::from(low) + f64::from(high)) / 2.0)
    }
}

/// The middle of where a part plays, as a key number (60 is middle C, 60.5 half-way to the next key): half-way
/// between its lowest and its highest key, strays aside (the keys at either end that sound for less than a twentieth
/// of its time between them). None when it has no notes that sound.
pub fn middle(notes: &[NoteEvent]) -> Option<f64> {
    let mut sounding = Sounding::NONE;
    sounding.add(notes);
    sounding.middle()
}

/// Where each kind of part plays in a song: the [`middle`] of all the song's parts of that kind together.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Registers {
    /// By kind, in [`Role::ALL`]'s order.
    of: [Option<f64>; Role::ALL.len()],
}

impl Registers {
    /// A song's: from every part that plays pitches. No drums, and no samples ([`crate::playing::Playing::sample`]:
    /// their notes say how far from the sample's own pitch they play, not which key sounds).
    pub fn of(song: &Song) -> Registers {
        let pitched = song.parts.iter().filter(|p| !p.drums && !p.sample);
        Registers::of_parts(pitched.map(|p| (p.role, p.notes.as_slice())))
    }

    /// A song's beside some of its bars (counted from 0), which a part found is fitted to: where each kind plays in
    /// those bars, and for a kind that rests there, where it plays in the rest of the song. A bass that's a low sub
    /// in the intro and a mid bass in the bars fitted to counts as the mid bass.
    pub fn by_bars(song: &Song, bars: &Range<i64>) -> Registers {
        let pitched = song.parts.iter().enumerate().filter(|(_, p)| !p.drums && !p.sample);
        let there: Vec<(Role, Vec<NoteEvent>)> = pitched.map(|(at, p)| (p.role, song.stretch(at, bars))).collect();
        let mut registers = Registers::of_parts(there.iter().map(|(role, notes)| (*role, notes.as_slice())));
        for (in_bars, in_song) in registers.of.iter_mut().zip(Registers::of(song).of) {
            *in_bars = in_bars.or(in_song);
        }
        registers
    }

    /// From parts given with their kinds.
    pub fn of_parts<'a>(parts: impl IntoIterator<Item = (Role, &'a [NoteEvent])>) -> Registers {
        let mut sounding = [Sounding::NONE; Role::ALL.len()];
        // Drums have no place: their notes pick pads.
        for (role, notes) in parts.into_iter().filter(|(role, _)| *role != Role::Drums) {
            if let Some(of_kind) = sounding.get_mut(place(role)) {
                of_kind.add(notes);
            }
        }
        let mut registers = Registers::default();
        for (of, sounding) in registers.of.iter_mut().zip(&sounding) {
            *of = sounding.middle();
        }
        registers
    }

    /// Where this kind of part plays in the song; none when the song has none of the kind.
    pub fn of_kind(&self, role: Role) -> Option<f64> {
        self.of.get(place(role)).copied().flatten()
    }

    /// Where the middle of a part of this kind belongs beside the song: its kind's usual place; for a kind that
    /// would stand in for the song's own ([`stands_in`]), half-way from there to where the song's plays, when it has
    /// one. None for drums.
    pub fn wanted(&self, role: Role) -> Option<f64> {
        let usual = usual(role)?;
        let in_song = self.of_kind(role).filter(|_| stands_in(role));
        Some(in_song.map_or(usual, |in_song| (usual + in_song) / 2.0))
    }

    /// How many octaves to move a part of this kind whose middle (moved to its key already) is `middle`, to bring it
    /// to [`Registers::wanted`] ([`octaves_to`]). 0 for drums.
    pub fn octaves(&self, role: Role, middle: f64) -> i32 {
        self.wanted(role).map_or(0, |wanted| octaves_to(wanted, middle))
    }

    /// The octaves a part of this kind goes at beside the song, once it's moved `semitones` into key
    /// ([`Registers::octaves`], by its [`middle`]), and never so many that the part leaves the keyboard: a part too
    /// wide to go where its kind belongs goes an octave short of it. That's the part itself, strays aside: a key
    /// switch far under it may still have no key to go to. None when it has no notes that sound.
    pub fn octaves_for(&self, role: Role, notes: &[NoteEvent], semitones: i32) -> Option<i32> {
        let mut sounding = Sounding::NONE;
        sounding.add(notes);
        let (low, high) = sounding.edges()?;
        let (low, high) = (i32::from(low) + semitones, i32::from(high) + semitones);
        let on_keyboard =
            |octaves: i32| low + 12 * octaves >= 0 && high + 12 * octaves <= i32::from(NoteEvent::TOP_KEY);
        let mut octaves = self.octaves(role, f64::from(low + high) / 2.0);
        while octaves != 0 && !on_keyboard(octaves) {
            octaves -= octaves.signum();
        }
        Some(octaves)
    }
}

/// Whether a part of this kind found for a song would stand in for the song's own part of the kind, and so goes
/// towards where that plays: a bassline and chords (a song has one bass register, and one for its chords). A tune,
/// an arp or anything else is a line beside the song's own, and keeps to its kind's usual place.
pub fn stands_in(role: Role) -> bool {
    matches!(role, Role::Bass | Role::Chords)
}

/// How many octaves to move a part whose middle is `middle` so it's by `wanted`: up for more than 0,
/// [`MOST_OCTAVES`] at the most either way. The fewest that bring it within half an octave; a part exactly half an
/// octave out stays.
pub fn octaves_to(wanted: f64, middle: f64) -> i32 {
    let away = wanted - middle;
    let octaves = ((away.abs() - 6.0) / 12.0).ceil().clamp(0.0, f64::from(MOST_OCTAVES)) as i32;
    if away < 0.0 { -octaves } else { octaves }
}

/// A kind's place in [`Registers::of`].
fn place(role: Role) -> usize {
    Role::ALL.iter().position(|r| *r == role).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_formats::NoteAttribute;

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

    /// Quarter notes on `keys`, one after the other.
    fn line(keys: &[u8]) -> Vec<NoteEvent> {
        keys.iter().zip(0i64..).map(|(&key, at)| note(at * PPQ, PPQ, key)).collect()
    }

    #[test]
    fn the_middle_is_half_way_between_the_keys_that_sound() {
        assert_eq!(middle(&line(&[60, 64, 67, 72])), Some(66.0));
        assert_eq!(middle(&line(&[60])), Some(60.0));
        assert_eq!(middle(&line(&[60, 61])), Some(60.5));
        assert_eq!(middle(&[]), None);
        // A bassline that jumps an octave is in the middle of the two, whichever it plays a little more.
        assert_eq!(middle(&line(&[36, 48, 36, 48, 36])), Some(42.0));
        // Muted notes aren't heard.
        let muted = NoteEvent { muted: true, ..note(0, PPQ, 20) };
        assert_eq!(middle(&[muted, note(PPQ, PPQ, 60), note(2 * PPQ, PPQ, 64)]), Some(62.0));
        assert_eq!(middle(&[muted]), None);
    }

    #[test]
    fn a_stray_note_doesnt_move_the_middle() {
        // 31 notes from C4 to G4, and one key switch four octaves down: under a twentieth of the time.
        let mut tune: Vec<u8> = (0..31).map(|at| 60 + (at % 8)).collect();
        assert_eq!(middle(&line(&tune)), Some(63.5));
        tune.push(12);
        assert_eq!(middle(&line(&tune)), Some(63.5));
        // The same at the top.
        tune.pop();
        tune.push(120);
        assert_eq!(middle(&line(&tune)), Some(63.5));
        // A tenth of the time isn't a stray: it's part of the part.
        tune.extend([120, 120, 120]);
        assert_eq!(middle(&line(&tune)), Some(90.0));
        // The two ends go by one rule: a note that's exactly a twentieth of the time counts, above or below.
        let mostly = |one: u8| -> Vec<u8> { std::iter::repeat_n(60, 19).chain([one]).collect() };
        assert_eq!((middle(&line(&mostly(72))), middle(&line(&mostly(48)))), (Some(66.0), Some(54.0)));
        // And one that's less doesn't, above or below.
        let mostly = |one: u8| -> Vec<u8> { std::iter::repeat_n(60, 20).chain([one]).collect() };
        assert_eq!((middle(&line(&mostly(72))), middle(&line(&mostly(48)))), (Some(60.0), Some(60.0)));
    }

    #[test]
    fn a_long_note_counts_for_no_more_than_8_beats_and_a_short_one_for_a_beat() {
        // A drone of 400 beats on C2 under 16 quarter notes on C5: the drone is a third of the time (8 of 24). Taken
        // whole it would be nearly all of it, and the tune a stray.
        let mut notes = vec![note(0, 400 * PPQ, 36)];
        notes.extend((0..16).map(|at| note(at * PPQ, PPQ, 84)));
        assert_eq!(middle(&notes), Some(60.0));
        // Two sixteenth notes among three held ones count a beat each (2 of 26): taken as they are (half a beat of
        // 24 and a half) they'd be strays.
        let mut notes: Vec<NoteEvent> = (0..3).map(|at| note(at * 8 * PPQ, 8 * PPQ, 36)).collect();
        notes.extend((0..2).map(|at| note(at * PPQ / 4, PPQ / 4, 84)));
        assert_eq!(middle(&notes), Some(60.0));
    }

    #[test]
    fn a_part_goes_to_the_octave_nearest_its_kinds_usual_place() {
        let none = Registers::default();
        assert_eq!(none.wanted(Role::Melody), Some(72.0));
        // Within half an octave of the place: as it is. Exactly half an octave out stays too.
        for middle in [66.0, 70.0, 72.0, 78.0] {
            assert_eq!(none.octaves(Role::Melody, middle), 0, "{middle}");
        }
        assert_eq!(none.octaves(Role::Melody, 65.9), 1);
        assert_eq!(none.octaves(Role::Melody, 78.1), -1);
        assert_eq!(none.octaves(Role::Melody, 48.0), 2);
        assert_eq!(none.octaves(Role::Melody, 54.0), 1, "a tune at 66 is near enough");
        assert_eq!(none.octaves(Role::Melody, 53.9), 2);
        // A bassline saved two octaves up comes down.
        assert_eq!(none.octaves(Role::Bass, 64.0), -2);
        assert_eq!(none.octaves(Role::Bass, 40.0), 0);
        // Never further than three octaves, and drums not at all.
        assert_eq!(none.octaves(Role::Bass, 127.0), -MOST_OCTAVES);
        assert_eq!(none.octaves(Role::Melody, 0.0), MOST_OCTAVES);
        assert_eq!((none.wanted(Role::Drums), none.octaves(Role::Drums, 0.0)), (None, 0));
        // Every pitched kind has a place.
        for role in Role::ALL {
            assert_eq!(usual(role).is_some(), role != Role::Drums, "{role:?}");
        }
    }

    #[test]
    fn the_octave_goes_by_the_part_in_key_and_keeps_it_on_the_keyboard() {
        let none = Registers::default();
        // A chord saved around middle C, as a bassline: two octaves down, to a bass's place.
        assert_eq!(none.octaves_for(Role::Bass, &line(&[60, 64, 67]), 0), Some(-2));
        // By where it is once it's in key: a tune whose middle is 66 is just near enough to 72; a semitone down
        // for the key it isn't, and goes up.
        let tune = line(&[64, 68]);
        assert_eq!(
            (none.octaves_for(Role::Melody, &tune, 0), none.octaves_for(Role::Melody, &tune, -1)),
            (Some(0), Some(1))
        );
        // A part too wide to go where its kind belongs goes an octave short rather than off the keyboard: this one
        // (its middle is 60) would go two octaves down as a bassline, and its low notes have only one to go.
        assert_eq!(none.octaves_for(Role::Bass, &line(&[20, 100]), 0), Some(-1));
        assert_eq!(none.octaves_for(Role::Bass, &line(&[20, 100]), -5), Some(-1));
        assert_eq!(none.octaves_for(Role::Bass, &line(&[5, 115]), 0), Some(0));
        // The same at the top: an octave up would take its high notes past the last key.
        assert_eq!(none.octaves_for(Role::Melody, &line(&[0, 110]), 0), Some(1));
        assert_eq!(none.octaves_for(Role::Melody, &line(&[0, 118]), 0), Some(0));
        // A stray doesn't hold the part back (it's the one note that may have no key to go to).
        let mut keys: Vec<u8> = (0..31).map(|at| 60 + (at % 8)).collect();
        keys.push(5);
        assert_eq!(none.octaves_for(Role::Bass, &line(&keys), 0), Some(-2));
        // Nothing that sounds: no octave. Drums: none.
        assert_eq!(none.octaves_for(Role::Bass, &[], 0), None);
        assert_eq!(none.octaves_for(Role::Drums, &line(&[100, 110]), 0), Some(0));
    }

    #[test]
    fn the_songs_own_bass_and_chords_pull_half_way_and_its_own_lines_dont() {
        // A song with a low sub bass (middle 30) and chords above middle C (middle 67).
        let (sub, keys) = (line(&[28, 30, 32, 30]), line(&[62, 65, 69, 72]));
        let registers = Registers::of_parts([(Role::Bass, sub.as_slice()), (Role::Chords, keys.as_slice())]);
        assert_eq!((registers.of_kind(Role::Bass), registers.of_kind(Role::Chords)), (Some(30.0), Some(67.0)));
        assert_eq!(registers.of_kind(Role::Melody), None);
        // A bassline belongs half-way between a bass's usual place (40) and the song's (30); chords likewise.
        assert_eq!((registers.wanted(Role::Bass), registers.wanted(Role::Chords)), (Some(35.0), Some(62.5)));
        // One at 42 would stay by the usual place alone; beside this song it goes down to 30, by the sub.
        assert_eq!((Registers::default().octaves(Role::Bass, 42.0), registers.octaves(Role::Bass, 42.0)), (0, -1));
        // The song has no tune: one found goes by a tune's usual place, wherever the chords and the bass are.
        assert_eq!(registers.wanted(Role::Melody), Some(72.0));
        // And when the song has a tune, a low one, an arp and a part of no kind: a line found keeps to its kind's
        // usual place all the same (two lines keep out of each other's way).
        let low = line(&[55, 57, 59, 60]);
        let lines = Registers::of_parts([Role::Melody, Role::Arp, Role::Other].map(|role| (role, low.as_slice())));
        assert_eq!(lines.of_kind(Role::Melody), Some(57.5));
        for role in [Role::Melody, Role::Arp, Role::Other] {
            assert_eq!((lines.wanted(role), stands_in(role)), (usual(role), false), "{role:?}");
        }
        assert!(stands_in(Role::Bass) && stands_in(Role::Chords) && !stands_in(Role::Drums));
        // Two parts of a kind count together.
        let high = line(&[40, 42, 44, 42]);
        let both = Registers::of_parts([(Role::Bass, sub.as_slice()), (Role::Bass, high.as_slice())]);
        assert_eq!(both.of_kind(Role::Bass), Some(36.0));
    }

    /// A song's part: quarter notes on `keys`, one after the other from its first beat.
    fn part(role: Role, keys: &[u8]) -> crate::playing::Playing {
        crate::playing::Playing {
            name: String::new(),
            plugin: String::new(),
            notes: line(keys),
            drums: false,
            sample: false,
            role,
            role_why: String::new(),
            bars: 0..1,
            clips: Vec::new(),
        }
    }

    fn song(parts: Vec<crate::playing::Playing>) -> Song {
        Song { view: "", tempo: 120.0, beats_per_bar: 4, parts, markers: Vec::new(), warnings: Vec::new() }
    }

    #[test]
    fn a_songs_registers_leave_out_drums_and_samples() {
        use crate::playing::Playing;
        let song = song(vec![
            part(Role::Bass, &[36, 38]),
            Playing { drums: true, ..part(Role::Bass, &[60, 62]) },
            Playing { sample: true, ..part(Role::Melody, &[60, 62]) },
            part(Role::Drums, &[36, 38]),
        ]);
        for registers in [Registers::of(&song), Registers::by_bars(&song, &(0..1))] {
            assert_eq!(registers.of_kind(Role::Bass), Some(37.0));
            assert_eq!(registers.of_kind(Role::Melody), None);
            assert_eq!(registers.of_kind(Role::Drums), None);
        }
    }

    #[test]
    fn beside_the_bars_fitted_to_a_kind_counts_where_it_plays_there() {
        // A bass that's a low sub in bar 1 and a mid bass in bar 3, and a tune that only plays in bar 1.
        let mut bass = part(Role::Bass, &[28, 30, 28, 30]);
        bass.notes.extend(line(&[43, 45, 43, 45]).into_iter().map(|n| NoteEvent { start: n.start + 2 * 4 * PPQ, ..n }));
        let song = song(vec![bass, part(Role::Melody, &[72, 74, 76, 74])]);
        // The whole song's bass is the two together.
        assert_eq!(Registers::of(&song).of_kind(Role::Bass), Some(36.5));
        // Beside bar 3 it's the mid bass; the tune rests there, and counts as it plays in the rest of the song.
        let third = Registers::by_bars(&song, &(2..3));
        assert_eq!((third.of_kind(Role::Bass), third.of_kind(Role::Melody)), (Some(44.0), Some(74.0)));
        // Beside bar 1 it's the sub.
        assert_eq!(Registers::by_bars(&song, &(0..1)).of_kind(Role::Bass), Some(29.0));
        // Beside a bar nothing plays in, everything counts as in the whole song.
        assert_eq!(Registers::by_bars(&song, &(5..6)), Registers::of(&song));
    }
}
