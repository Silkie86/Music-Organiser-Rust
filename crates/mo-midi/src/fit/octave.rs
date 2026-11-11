//! The octave a part found goes at beside a song.
//!
//! A search says how many semitones to move a part so its notes go with the song's ([`super::Found::sits`]). That
//! settles its key and leaves its octave open: a bassline saved two octaves up is in key all the same. This says the
//! octave: the one that brings the middle of the part ([`middle`]) nearest to where a part of its kind belongs
//! beside the song ([`Registers::wanted`]), which is half-way between where that kind of part usually plays
//! ([`usual`]) and where it plays in the song, or the usual place when the song has none of its kind.
//!
//! Measured on POP909-CL's 906 songs, 8 bars at a time (`theorybench octave`, `docs/harmony/together-experiments.md`):
//! how often a part taken out of its song is put back at the octave it was written at. No rule gets past about 3 in
//! 4, because the octave a tune is written at is partly taste, so the page lets the octave be changed by hand as
//! well. What the numbers do say:
//!
//! - The usual place alone does as well as anything. Where the song's other kinds of part play says next to
//!   nothing about this one (the C# app nudged the usual place by them: no better for a tune, worse for chords), so
//!   they aren't asked.
//! - A second tune written to go with a song's first is less often at the first tune's height than at a tune's usual
//!   height, so the song's own part of the kind only pulls half-way: a found bassline still lands by the song's low
//!   bass, and a found tune isn't dragged down to a song's one low melody.
//! - Which middle is taken matters little; this one isn't thrown by a stray note (a key switch far below the part).

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

/// The share of a part's sounding time taken to be strays at its low end, and at its high end.
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

    /// The middle of the keys that sound, [`STRAYS`] of the time at each end aside; none when nothing sounds.
    fn middle(&self) -> Option<f64> {
        let all: f64 = self.0.iter().sum();
        if all <= 0.0 {
            return None;
        }
        // The first key by which this much of the time has sounded, counting up from the lowest.
        let key_at = |share: f64| {
            let mut so_far = 0.0;
            self.0.iter().zip(0u8..).find_map(|(time, key)| {
                so_far += time;
                (*time > 0.0 && so_far >= share * all).then_some(f64::from(key))
            })
        };
        // Both are there: by the highest key that sounds, all of the time has (it's added up in the same order).
        Some((key_at(STRAYS)? + key_at(1.0 - STRAYS)?) / 2.0)
    }
}

/// The middle of where a part plays, as a key number (60 is middle C, 60.5 half-way to the next key): half-way
/// between its lowest and its highest key, the lowest and the highest twentieth of its sounding time aside. None when
/// it has no notes that sound.
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

    /// Where the middle of a part of this kind belongs beside the song: half-way between the kind's usual place and
    /// where it plays in the song; the usual place when the song has none of it. None for drums.
    pub fn wanted(&self, role: Role) -> Option<f64> {
        let usual = usual(role)?;
        Some(self.of_kind(role).map_or(usual, |in_song| (usual + in_song) / 2.0))
    }

    /// How many octaves to move a part of this kind whose middle (moved to its key already) is `middle`, to bring it
    /// to [`Registers::wanted`] ([`octaves_to`]). 0 for drums.
    pub fn octaves(&self, role: Role, middle: f64) -> i32 {
        self.wanted(role).map_or(0, |wanted| octaves_to(wanted, middle))
    }
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
    fn the_songs_own_part_of_the_kind_pulls_half_way() {
        // A song with a low sub bass (middle 30) and chords by middle C.
        let (sub, keys) = (line(&[28, 30, 32, 30]), line(&[55, 60, 64, 67]));
        let registers = Registers::of_parts([(Role::Bass, sub.as_slice()), (Role::Chords, keys.as_slice())]);
        assert_eq!((registers.of_kind(Role::Bass), registers.of_kind(Role::Chords)), (Some(30.0), Some(61.0)));
        assert_eq!(registers.of_kind(Role::Melody), None);
        // A bassline belongs half-way between a bass's usual place (40) and the song's (30).
        assert_eq!(registers.wanted(Role::Bass), Some(35.0));
        // One at 42 would stay by the usual place alone; beside this song it goes down to 30, by the sub.
        assert_eq!((Registers::default().octaves(Role::Bass, 42.0), registers.octaves(Role::Bass, 42.0)), (0, -1));
        // The song has no tune: one found goes by a tune's usual place, wherever the chords and the bass are.
        assert_eq!(registers.wanted(Role::Melody), Some(72.0));
        // Two parts of a kind count together.
        let high = line(&[40, 42, 44, 42]);
        let both = Registers::of_parts([(Role::Bass, sub.as_slice()), (Role::Bass, high.as_slice())]);
        assert_eq!(both.of_kind(Role::Bass), Some(36.0));
    }

    #[test]
    fn a_songs_registers_leave_out_drums_and_samples() {
        use crate::playing::Playing;
        let part = |role: Role, keys: &[u8]| Playing {
            name: String::new(),
            plugin: String::new(),
            notes: line(keys),
            drums: false,
            sample: false,
            role,
            role_why: String::new(),
            bars: 0..1,
            clips: Vec::new(),
        };
        let song = Song {
            view: "",
            tempo: 120.0,
            beats_per_bar: 4,
            parts: vec![
                part(Role::Bass, &[36, 38]),
                Playing { drums: true, ..part(Role::Bass, &[60, 62]) },
                Playing { sample: true, ..part(Role::Melody, &[60, 62]) },
                part(Role::Drums, &[36, 38]),
            ],
            markers: Vec::new(),
            warnings: Vec::new(),
        };
        let registers = Registers::of(&song);
        assert_eq!(registers.of_kind(Role::Bass), Some(37.0));
        assert_eq!(registers.of_kind(Role::Melody), None);
        assert_eq!(registers.of_kind(Role::Drums), None);
    }
}
