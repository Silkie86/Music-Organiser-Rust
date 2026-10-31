//! The key finder's tests: what it hears in the notes (exact counts), and the rules every answer keeps whatever the
//! weights are (moving the notes moves the answer exactly; the order of the notes, how they're split into parts, a
//! loop written out again, octave doubling, ghost notes, velocities and drums change nothing; bars heard one by one
//! add up to the whole; damaged notes don't stop it).

use super::*;
use crate::chord::Chord;
use crate::notes::{Note, Role};
use crate::pitch::PerPitch;

/// Ticks in a beat, as most DAWs save them.
const TPB: u32 = 96;
const BEAT: i64 = TPB as i64;
/// A MIDI 1.0 velocity of 100 in sixteen bits.
const LOUD: u16 = 100 << 9;

fn note(beat: f64, beats: f64, key: u8) -> Note {
    Note { start: (beat * BEAT as f64) as i64, length: (beats * BEAT as f64) as i64, key, velocity: LOUD }
}

fn pc(n: i32) -> PitchClass {
    PitchClass::new(n)
}

/// A piece in three parts.
#[derive(Clone, Default)]
struct Piece {
    melody: Vec<Note>,
    harmony: Vec<Note>,
    bass: Vec<Note>,
}

impl Piece {
    /// One chord a bar (four beats), held in the harmony part (close position from middle C up) with its bass note
    /// on every beat, and the tune's notes one after another, each `(MIDI key, beats)`.
    fn new(chords: &[&str], tune: &[(u8, f64)]) -> Piece {
        let mut piece = Piece::default();
        for (bar, name) in chords.iter().enumerate() {
            let chord = Chord::parse(name).unwrap_or_else(|| panic!("{name} isn't a chord"));
            let at = bar as f64 * 4.0;
            for pc in chord.sounding().iter() {
                piece.harmony.push(note(at, 4.0, 60 + pc.number()));
            }
            for b in 0..4 {
                piece.bass.push(note(at + f64::from(b), 1.0, 36 + chord.bass().number()));
            }
        }
        let mut at = 0.0;
        for &(k, beats) in tune {
            piece.melody.push(note(at, beats, k));
            at += beats;
        }
        piece
    }

    fn all(&self) -> [&Vec<Note>; 3] {
        [&self.melody, &self.harmony, &self.bass]
    }

    fn map(&self, f: impl Fn(&[Note]) -> Vec<Note>) -> Piece {
        Piece { melody: f(&self.melody), harmony: f(&self.harmony), bass: f(&self.bass) }
    }

    /// The same piece played this many times in a row.
    fn repeated(&self, times: i64) -> Piece {
        let end = self.all().iter().flat_map(|p| p.iter().map(|n| n.end())).max().unwrap_or(0);
        let bar = 4 * BEAT;
        let length = (end + bar - 1) / bar * bar;
        self.map(|notes| {
            (0..times).flat_map(|t| notes.iter().map(move |n| Note { start: n.start + t * length, ..*n })).collect()
        })
    }

    /// Every note this many semitones up (down when negative).
    fn transposed(&self, semitones: i32) -> Piece {
        self.map(|notes| notes.iter().map(|n| Note { key: (i32::from(n.key) + semitones) as u8, ..*n }).collect())
    }

    fn parts(&self) -> [Part<'_>; 3] {
        [
            Part { role: Role::Melody, notes: &self.melody },
            Part { role: Role::Harmony, notes: &self.harmony },
            Part { role: Role::Bass, notes: &self.bass },
        ]
    }

    fn heard(&self) -> Heard {
        hear(&self.parts(), Grid::four_four(TPB))
    }
}

fn hear(parts: &[Part], grid: Grid) -> Heard {
    KeyReader::new().hear(parts, grid).unwrap()
}

/// Each feature and its value, in the order they're given.
fn features(heard: &Heard, key: Key, span: Span) -> Vec<(usize, f64)> {
    let mut out = Vec::new();
    heard.features(key, span, |i, v| out.push((i, v)));
    out
}

/// Weights with no musical sense but no pattern either, for the rules that hold whatever the weights are.
fn odd_weights() -> [f64; FEATURES] {
    std::array::from_fn(|i| ((i * 7919 % 1013) as f64 / 1013.0 - 0.5) * 4.0)
}

const ODD_CALIBRATION: Calibration =
    Calibration([0.6, 0.8, 1.1, 1.3, 1.6, -0.2, -0.3, -0.1, 0.4, 0.2, 0.1, -0.1, -0.2, 0.3]);

fn odd_chances(heard: &Heard, span: Span) -> [f64; 24] {
    read_heard(heard, span, &Model { weights: &odd_weights(), calibration: &ODD_CALIBRATION }).chances
}

/// A piece with a tune, chords that move and a bass: C minor, with a raised seventh.
fn c_minor() -> Piece {
    let tune = [
        (67, 1.0),
        (68, 0.5),
        (67, 0.5),
        (65, 1.0),
        (63, 1.0),
        (60, 2.0),
        (63, 2.0),
        (65, 1.5),
        (68, 0.5),
        (72, 2.0),
        (71, 1.0),
        (74, 1.0),
        (67, 2.0),
    ];
    Piece::new(&["Cm", "Ab", "Fm", "G7"], &tune)
}

// What the finder hears.

#[test]
fn a_held_chord_is_heard_beat_by_beat() {
    let heard = Piece::new(&["C"], &[]).heard();
    let (c, e, g) = (PitchClass::C, PitchClass::E, PitchClass::G);
    // The chord's three notes held for 4 beats, and the bass's C on each beat.
    assert_eq!((heard.harmony[c], heard.harmony[e], heard.harmony[g]), (768, 384, 384));
    assert_eq!(heard.tune, PerPitch([0; 12]));
    // The bass on each beat, weighted 4, 1, 2, 1 by its place in the bar, with a C major chord standing on it.
    assert_eq!((heard.bass[c], heard.major_on_bass[c], heard.minor_on_bass[c], heard.flat7_on_bass[c]), (8, 8, 0, 0));
    assert_eq!((heard.first_bass, heard.last_bass), (Some(c), Some(c)));
    assert_eq!((heard.notes, heard.bars_heard(), heard.tune_beats(), heard.accompaniment_beats()), (7, 1, 0, 4));
    assert!(heard.roles.has(Role::Harmony) && heard.roles.has(Role::Bass) && !heard.roles.has(Role::Melody));
    assert_eq!((heard.config(), heard.diatonic_fit()), (Config::Accompaniment, 1.0));
}

#[test]
fn a_beat_counts_by_its_place_in_the_bar() {
    // A different bass note on each beat of one bar.
    let weights = |beats_per_bar: u32| {
        let bass: Vec<Note> = (0..beats_per_bar).map(|b| note(f64::from(b), 1.0, 36 + b as u8)).collect();
        let grid = Grid { ticks_per_beat: TPB, beats_per_bar, bar_line: 0 };
        let heard = hear(&[Part { role: Role::Bass, notes: &bass }], grid);
        (0..beats_per_bar as i32).map(|b| heard.bass[pc(b)]).collect::<Vec<_>>()
    };
    assert_eq!(weights(4), [4, 1, 2, 1]);
    assert_eq!(weights(3), [4, 1, 1]);
    assert_eq!(weights(5), [4, 1, 1, 1, 1]);
    assert_eq!(weights(2), [4, 1]);
    // 6/8, 9/8 and 12/8, counted in eighths: every third eighth is a beat.
    assert_eq!(weights(6), [4, 1, 1, 2, 1, 1]);
    assert_eq!(weights(9), [4, 1, 1, 2, 1, 1, 2, 1, 1]);
    assert_eq!(weights(12), [4, 1, 1, 2, 1, 1, 2, 1, 1, 2, 1, 1]);

    // With the bar lines two beats before the music starts, its first beat is the third of a bar: where the music
    // starts doesn't matter, only the bar.
    let bass: Vec<Note> = (0..4).map(|b| note(f64::from(b), 1.0, 36 + b as u8)).collect();
    let grid = Grid { ticks_per_beat: TPB, beats_per_bar: 4, bar_line: -2 * BEAT };
    let heard = hear(&[Part { role: Role::Bass, notes: &bass }], grid);
    assert_eq!((0..4).map(|b| heard.bass[pc(b)]).collect::<Vec<_>>(), [2, 1, 4, 1]);
    assert_eq!((heard.first_bass, heard.last_bass), (Some(PitchClass::D), Some(PitchClass::D)));
    // The bars are counted from the bar line too: two bars, both different.
    assert_eq!(heard.bars_heard(), 2);
}

/// The bass heard under these parts, weighted by place in the bar, by pitch class.
fn bass_of(parts: &[Part]) -> Vec<(PitchClass, u64)> {
    let heard = hear(parts, Grid::four_four(TPB));
    heard.bass.iter().filter(|(_, n)| *n > 0).collect()
}

#[test]
fn the_bass_of_a_beat() {
    let chord = |keys: &[u8]| keys.iter().map(|&k| note(0.0, 1.0, k)).collect::<Vec<_>>();
    let harmony = |keys: &[u8]| bass_of(&[Part { role: Role::Harmony, notes: &chord(keys) }]);
    // The lowest note of the chords is the bass when nothing sounds within a fourth above it.
    assert_eq!(harmony(&[48, 64, 67]), [(PitchClass::C, 4)]);
    assert_eq!(harmony(&[48, 55]), [(PitchClass::C, 4)]);
    assert_eq!(harmony(&[48, 53]), [(PitchClass::C, 4)]);
    assert_eq!(harmony(&[48]), [(PitchClass::C, 4)]);
    assert_eq!(harmony(&[48, 60, 72]), [(PitchClass::C, 4)]);
    // Close voicings and clusters have no bass: any of their notes could be heard as the root.
    assert_eq!(harmony(&[48, 52, 55]), []);
    assert_eq!(harmony(&[48, 51]), []);
    // How high the notes are doesn't matter, only the gaps.
    assert_eq!(harmony(&[96, 112, 115]), [(PitchClass::C, 4)]);
    assert_eq!(harmony(&[12, 16, 19]), []);

    // A bass part is the bass, even with a chord note below it.
    let low = chord(&[36, 60]);
    let bass = chord(&[40]);
    let parts = [Part { role: Role::Harmony, notes: &low }, Part { role: Role::Bass, notes: &bass }];
    assert_eq!(bass_of(&parts), [(PitchClass::E, 4)]);
    // The tune is never the bass, however low.
    let tune = chord(&[28]);
    let a_minor = chord(&[45, 60, 64]);
    let parts = [Part { role: Role::Melody, notes: &tune }, Part { role: Role::Harmony, notes: &a_minor }];
    assert_eq!(bass_of(&parts), [(PitchClass::A, 4)]);
}

#[test]
fn a_bass_note_counts_on_the_beats_it_starts_or_is_held_into() {
    let bass_part = |notes: &[Note]| bass_of(&[Part { role: Role::Bass, notes }]);
    // Pushed ahead of the second beat and held over it: it's the second beat's bass (weight 1), not the first's.
    assert_eq!(bass_part(&[note(0.75, 1.25, 36)]), [(PitchClass::C, 1)]);
    // An off-beat note starting halfway through the first beat: no beat's bass.
    assert_eq!(bass_part(&[note(0.5, 0.5, 36)]), []);
    // A note starting a quarter of the way in: the first beat's bass.
    assert_eq!(bass_part(&[note(0.25, 0.25, 36)]), [(PitchClass::C, 4)]);
    // Played legato, a bass note held a moment into the next beat isn't that beat's bass: the next note is.
    assert_eq!(bass_part(&[note(0.0, 1.0625, 36), note(1.0, 1.0, 40)]), [(PitchClass::C, 4), (PitchClass::E, 1)]);
    // Nor is the last moment of a low chord note the next chord's bass.
    let legato = [note(0.0, 1.0625, 36), note(1.0, 1.0, 43), note(1.0, 1.0, 55)];
    assert_eq!(bass_of(&[Part { role: Role::Harmony, notes: &legato }]), [(PitchClass::C, 4), (PitchClass::G, 1)]);
}

#[test]
fn chords_standing_on_the_bass() {
    // Whole notes in a bar of 4/4: bass weights 4 + 1 + 2 + 1.
    let heard = |bass: u8, chord: &[u8]| {
        let bass = [note(0.0, 4.0, bass)];
        let chord: Vec<Note> = chord.iter().map(|&k| note(0.0, 4.0, k)).collect();
        hear(
            &[Part { role: Role::Harmony, notes: &chord }, Part { role: Role::Bass, notes: &bass }],
            Grid::four_four(TPB),
        )
    };
    let c = PitchClass::C;
    // C7♯9: the minor third over a major third is a sharp ninth, so C major stands on the bass, and its flat 7th.
    let h = heard(36, &[64, 67, 70, 75]);
    assert_eq!((h.major_on_bass[c], h.minor_on_bass[c], h.flat7_on_bass[c]), (8, 0, 8));
    let h = heard(36, &[63, 67]);
    assert_eq!((h.major_on_bass[c], h.minor_on_bass[c], h.flat7_on_bass[c]), (0, 8, 0));
    // C over E: no chord stands on E.
    let h = heard(40, &[60, 64, 67]);
    assert_eq!((h.bass[PitchClass::E], h.major_on_bass, h.minor_on_bass), (8, PerPitch([0; 12]), PerPitch([0; 12])));
    // A chord note counts when it sounds for at least a quarter of the beat.
    let bass = [note(0.0, 4.0, 36)];
    let mut chord = vec![note(0.0, 4.0, 67)];
    chord.extend((0..4).map(|b| Note { start: b * BEAT, length: BEAT / 4, key: 64, velocity: LOUD }));
    let parts = [Part { role: Role::Harmony, notes: &chord }, Part { role: Role::Bass, notes: &bass }];
    assert_eq!(hear(&parts, Grid::four_four(TPB)).major_on_bass[c], 8);
    for n in chord.iter_mut().skip(1) {
        n.length = BEAT / 4 - 1;
    }
    let parts = [Part { role: Role::Harmony, notes: &chord }, Part { role: Role::Bass, notes: &bass }];
    assert_eq!(hear(&parts, Grid::four_four(TPB)).major_on_bass[c], 0);
}

#[test]
fn a_held_note_counts_for_two_bars_and_one_bar_heard() {
    let held: Vec<Note> = [48, 64, 67].iter().map(|&k| note(0.0, 16.0, k)).collect();
    let heard = hear(&[Part { role: Role::Harmony, notes: &held }], Grid::four_four(TPB));
    let (c, e, g) = (PitchClass::C, PitchClass::E, PitchClass::G);
    assert_eq!((heard.harmony[c], heard.harmony[e], heard.harmony[g]), (768, 768, 768));
    // C3 stands apart below E4 and G4: it's the bass of the 8 beats it's heard in.
    assert_eq!((heard.bass[c], heard.major_on_bass[c]), (16, 16));
    // One bar has notes starting in it, so one bar heard, of 4 beats.
    assert_eq!((heard.bars_heard(), heard.accompaniment_beats()), (1, 4));
}

#[test]
fn ghost_notes_are_left_out() {
    let piece = c_minor();
    let plain = piece.heard();
    // A 32nd of a beat at a 16th of the velocity, beside every note.
    let ghosts = piece.map(|notes| {
        notes
            .iter()
            .flat_map(|n| [*n, Note { start: n.start + BEAT / 8, length: 3, key: n.key + 1, velocity: LOUD / 16 }])
            .collect()
    });
    assert_eq!(ghosts.heard(), plain);
    // A short note played loud, or a quiet note held, is music.
    let mut loud = piece.clone();
    loud.melody.push(Note { start: 0, length: 3, key: 61, velocity: LOUD });
    assert_ne!(loud.heard(), plain);
    let mut held = piece.clone();
    held.melody.push(Note { start: 0, length: BEAT, key: 61, velocity: LOUD / 16 });
    assert_ne!(held.heard(), plain);
}

#[test]
fn diatonic_fit_tells_keys_from_scales_that_have_none() {
    let fit = |pcs: &[u8]| {
        let tune: Vec<Note> = (0..48usize).map(|i| note(i as f64 / 2.0, 0.5, 60 + pcs[i % pcs.len()])).collect();
        hear(&[Part { role: Role::Melody, notes: &tune }], Grid::four_four(TPB)).diatonic_fit()
    };
    assert_eq!(fit(&[0, 2, 4, 5, 7, 9, 11, 12]), 1.0);
    assert_eq!(fit(&[0, 2, 4, 6, 8, 10]), 4.0 / 6.0);
    assert_eq!(fit(&[0, 1, 3, 4, 6, 7, 9, 10]), 5.0 / 8.0);
    assert_eq!(fit(&(0..12).collect::<Vec<_>>()), 7.0 / 12.0);
}

#[test]
fn which_streams_have_sound() {
    let notes = [note(0.0, 1.0, 60), note(1.0, 1.0, 64), note(2.0, 1.0, 67)];
    let config = |roles: &[Role]| {
        let parts: Vec<Part> = roles.iter().map(|&role| Part { role, notes: &notes }).collect();
        hear(&parts, Grid::four_four(TPB)).config()
    };
    assert_eq!(config(&[Role::Melody]), Config::MelodyOnly);
    assert_eq!(config(&[Role::Bass]), Config::BassOnly);
    assert_eq!(config(&[Role::Harmony]), Config::Accompaniment);
    assert_eq!(config(&[Role::Unknown, Role::Bass]), Config::Accompaniment);
    assert_eq!(config(&[Role::Melody, Role::Bass]), Config::Full);
    assert_eq!(config(&[Role::Melody, Role::Drums]), Config::MelodyOnly);
}

#[test]
fn drums_and_silence_give_nothing() {
    let drums = [note(0.0, 1.0, 36), note(1.0, 1.0, 38)];
    let grid = Grid::four_four(TPB);
    assert_eq!(KeyReader::new().hear(&[Part { role: Role::Drums, notes: &drums }], grid), None);
    assert_eq!(KeyReader::new().hear(&[Part { role: Role::Melody, notes: &[] }], grid), None);
    assert_eq!(KeyReader::new().hear(&[], grid), None);
    assert_eq!(find_key(&[], grid, Span::Loop), None);
}

// What every reading keeps, whatever the weights.

#[test]
fn each_span_hears_its_own_ends() {
    let heard = c_minor().heard();
    let key = Key::minor(PitchClass::C);
    let ends = |span: Span| {
        features(&heard, key, span).into_iter().filter(|(i, _)| (5 * 24..MINOR).contains(i)).collect::<Vec<_>>()
    };
    // Starts on C (Cm) and ends on G (G7): a loop counts where it starts, a song both ends, a window neither.
    assert_eq!(ends(Span::Loop), [(5 * 24 + 12, 1.0)]);
    assert_eq!(ends(Span::Song), [(6 * 24 + 12, 1.0), (7 * 24 + 12 + 7, 1.0)]);
    assert_eq!(ends(Span::Window), []);
    // The minor keys' own weight, last.
    assert_eq!(features(&heard, key, Span::Window).last(), Some(&(MINOR, 1.0)));
    assert!(features(&heard, Key::major(PitchClass::C), Span::Window).iter().all(|(i, _)| *i < MINOR));
}

#[test]
fn a_stream_counts_more_the_more_beats_it_has() {
    // The tune of one beat, against chords of four.
    let tune = [note(0.0, 1.0, 72)];
    let chord = [note(0.0, 4.0, 60), note(0.0, 4.0, 64), note(0.0, 4.0, 67)];
    let parts = [Part { role: Role::Melody, notes: &tune }, Part { role: Role::Harmony, notes: &chord }];
    let f = features(&hear(&parts, Grid::four_four(TPB)), Key::major(PitchClass::C), Span::Window);
    // Tune: all of it on C, with 1 beat: 1/5. Chords: a third on C, with 4 beats: 4/8 · 1/3.
    assert_eq!(f[0], (0, 1.0 / 5.0));
    assert_eq!(f[1], (24, 0.5 * (1.0 / 3.0)));
}

#[test]
fn features_and_scores_agree() {
    let heard = c_minor().heard();
    let w = odd_weights();
    for span in Span::ALL {
        for key in Key::ALL {
            let mut by_hand = 0.0;
            for (i, v) in features(&heard, key, span) {
                by_hand += v * w[i];
            }
            assert_eq!(score(&heard, key, span, &w), by_hand);
        }
    }
}

#[test]
fn moving_the_notes_moves_everything_exactly() {
    let piece = c_minor();
    let heard = piece.heard();
    for s in -6..=5 {
        let moved = piece.transposed(s).heard();
        for (counts, moved_counts) in [
            (&heard.tune, &moved.tune),
            (&heard.harmony, &moved.harmony),
            (&heard.bass, &moved.bass),
            (&heard.major_on_bass, &moved.major_on_bass),
            (&heard.minor_on_bass, &moved.minor_on_bass),
            (&heard.flat7_on_bass, &moved.flat7_on_bass),
        ] {
            assert_eq!(moved_counts.rotated(pc(s)), *counts, "moved {s}");
        }
        let up = |p: Option<PitchClass>| p.map(|p| p.up(s));
        assert_eq!((moved.first_bass, moved.last_bass), (up(heard.first_bass), up(heard.last_bass)));
        let sizes = |h: &Heard| (h.bars_heard(), h.tune_beats(), h.accompaniment_beats(), h.notes);
        assert_eq!(sizes(&moved), sizes(&heard));
        for span in Span::ALL {
            for key in Key::ALL {
                let up = Key { tonic: key.tonic.up(s), mode: key.mode };
                assert_eq!(features(&moved, up, span), features(&heard, key, span), "moved {s}, {key:?}, {span:?}");
            }
            // The chances turn with the keys, to the last bit.
            let (before, after) = (odd_chances(&heard, span), odd_chances(&moved, span));
            for key in Key::ALL {
                let up = Key { tonic: key.tonic.up(s), mode: key.mode };
                assert_eq!(after[up.index()].to_bits(), before[key.index()].to_bits(), "moved {s}, {key:?}");
            }
        }
    }
}

#[test]
fn a_loop_written_out_again_is_the_same_loop() {
    let piece = c_minor();
    let once = piece.heard();
    let sizes = |h: &Heard| (h.bars_heard(), h.tune_beats(), h.accompaniment_beats());
    for times in [2, 4] {
        let again = piece.repeated(times).heard();
        let n = times as u64;
        assert_eq!(again.tune, PerPitch(once.tune.0.map(|c| c * n)));
        assert_eq!(again.bass, PerPitch(once.bass.0.map(|c| c * n)));
        assert_eq!(sizes(&again), sizes(&once));
        for span in Span::ALL {
            for key in Key::ALL {
                assert_eq!(features(&again, key, span), features(&once, key, span), "{times} times, {key:?}, {span:?}");
            }
            assert_eq!(odd_chances(&again, span).map(f64::to_bits), odd_chances(&once, span).map(f64::to_bits));
        }
    }
}

#[test]
fn order_parts_velocity_octaves_and_drums_change_nothing() {
    let piece = c_minor();
    let heard = piece.heard();
    let grid = Grid::four_four(TPB);

    // The notes backwards, and the parts in another order.
    let backwards = piece.map(|notes| notes.iter().rev().copied().collect());
    let [m, h, b] = backwards.parts();
    assert_eq!(hear(&[b, m, h], grid), heard);

    // The chords split between two parts, and a drum part added.
    let left: Vec<Note> = piece.harmony.iter().step_by(2).copied().collect();
    let right: Vec<Note> = piece.harmony.iter().skip(1).step_by(2).copied().collect();
    let drums: Vec<Note> = (0..64).map(|i| note(f64::from(i) / 4.0, 0.1, 36 + (i % 3) as u8)).collect();
    let split = [
        Part { role: Role::Melody, notes: &piece.melody },
        Part { role: Role::Harmony, notes: &left },
        Part { role: Role::Drums, notes: &drums },
        Part { role: Role::Harmony, notes: &right },
        Part { role: Role::Bass, notes: &piece.bass },
    ];
    assert_eq!(hear(&split, grid), heard);

    // Every velocity halved.
    let quiet = piece.map(|notes| notes.iter().map(|n| Note { velocity: n.velocity / 2, ..*n }).collect());
    assert_eq!(quiet.heard(), heard);

    // Every note doubled an octave up: twice the time, the same shares, the same bass and bars.
    let doubled = piece.map(|notes| notes.iter().flat_map(|n| [*n, Note { key: n.key + 12, ..*n }]).collect()).heard();
    for span in Span::ALL {
        for key in Key::ALL {
            assert_eq!(features(&doubled, key, span), features(&heard, key, span), "{key:?}, {span:?}");
        }
    }
}

/// A long piece in four parts with notes held over bar lines, made up by a fixed sum.
fn long_piece(bars: i64, beats_per_bar: i64) -> [Vec<Note>; 4] {
    let mut seed = 12345u64;
    let mut next = |below: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % below
    };
    let mut parts: [Vec<Note>; 4] = Default::default();
    let ticks = bars * beats_per_bar * BEAT;
    for (p, notes) in parts.iter_mut().enumerate() {
        let mut at = next(BEAT as u64) as i64;
        while at < ticks {
            let length = 1 + next(3 * BEAT as u64 * beats_per_bar as u64) as i64;
            let key = [36, 48, 60, 72][p] + next(12) as u8;
            let velocity = (1 + next(127) as u16) << 9;
            notes.push(Note { start: at, length, key, velocity });
            at += next(2 * BEAT as u64) as i64;
        }
    }
    parts
}

#[test]
fn bars_heard_one_by_one_add_up_to_the_whole() {
    for (beats_per_bar, bar_line) in [(4, 0), (3, -BEAT), (12, 5 * BEAT)] {
        let [bass, harmony, unknown, melody] = long_piece(64, i64::from(beats_per_bar));
        let parts = [
            Part { role: Role::Bass, notes: &bass },
            Part { role: Role::Harmony, notes: &harmony },
            Part { role: Role::Unknown, notes: &unknown },
            Part { role: Role::Melody, notes: &melody },
        ];
        let grid = Grid { ticks_per_beat: TPB, beats_per_bar, bar_line };
        let mut reader = KeyReader::new();
        let whole = reader.hear(&parts, grid).unwrap();
        let (first_bar, bars) = reader.hear_bars(&parts, grid).unwrap();
        assert!(grid.starts_bar(first_bar) && first_bar <= 0 && first_bar > -grid.bar());
        assert!(bars.len() >= 64);
        let mut sum = Heard::default();
        for bar in &bars {
            sum.merge(bar);
        }
        assert_eq!(sum, whole);
        // The first bass from the start, the last from the end.
        assert_eq!(sum.first_bass, bars.iter().find_map(|b| b.first_bass));
        assert_eq!(sum.last_bass, bars.iter().rev().find_map(|b| b.last_bass));
        // Any split in two adds up too.
        for cut in [1, 17, bars.len() - 1] {
            let (mut early, mut late) = (Heard::default(), Heard::default());
            bars[..cut].iter().for_each(|b| early.merge(b));
            bars[cut..].iter().for_each(|b| late.merge(b));
            early.merge(&late);
            assert_eq!(early, whole);
        }
    }
}

#[test]
fn a_reader_used_again_hears_the_same() {
    let mut reader = KeyReader::new();
    let [bass, harmony, unknown, melody] = long_piece(16, 4);
    let long = [
        Part { role: Role::Bass, notes: &bass },
        Part { role: Role::Harmony, notes: &harmony },
        Part { role: Role::Unknown, notes: &unknown },
        Part { role: Role::Melody, notes: &melody },
    ];
    let piece = c_minor();
    let grid = Grid::four_four(TPB);
    for _ in 0..2 {
        assert_eq!(reader.hear(&long, grid), KeyReader::new().hear(&long, grid));
        assert_eq!(reader.hear(&piece.parts(), grid), Some(piece.heard()));
        assert_eq!(reader.read(&piece.parts(), grid, Span::Loop), find_key(&piece.parts(), grid, Span::Loop));
    }
}

#[test]
fn damaged_notes_and_grids_give_an_answer_without_a_crash() {
    let weights = odd_weights();
    let model = Model { weights: &weights, calibration: &ODD_CALIBRATION };
    let notes = [
        Note { start: i64::MIN, length: -5, key: 127, velocity: 0 },
        Note { start: i64::MAX, length: i64::MAX, key: 255, velocity: u16::MAX },
        Note { start: -7, length: 0, key: 0, velocity: 1 },
        Note { start: 10_000_000 * BEAT, length: 3, key: 60, velocity: LOUD },
        Note { start: 0, length: i64::MIN, key: 64, velocity: LOUD },
        Note { start: 5, length: 1, key: 200, velocity: LOUD },
    ];
    let grids = [
        Grid { ticks_per_beat: 0, beats_per_bar: 0, bar_line: i64::MIN },
        Grid { ticks_per_beat: u32::MAX, beats_per_bar: u32::MAX, bar_line: i64::MAX },
        Grid { ticks_per_beat: 1, beats_per_bar: 1, bar_line: 3 },
        Grid::four_four(TPB),
    ];
    let mut reader = KeyReader::new();
    for grid in grids {
        for role in [Role::Melody, Role::Harmony, Role::Bass, Role::Unknown] {
            for take in 1..=notes.len() {
                let parts =
                    [Part { role, notes: &notes[..take] }, Part { role: Role::Bass, notes: &notes[take - 1..] }];
                let heard = reader.hear(&parts, grid).unwrap();
                for span in Span::ALL {
                    let reading = read_heard(&heard, span, &model);
                    let total: f64 = reading.chances.iter().sum();
                    assert!((total - 1.0).abs() < 1e-12 && reading.chances.iter().all(|p| p.is_finite() && *p >= 0.0));
                }
                let (_, bars) = reader.hear_bars(&parts, grid).unwrap();
                assert!(bars.len() <= MOST_BEATS);
            }
        }
    }
}

#[test]
fn a_stray_note_far_from_the_music_is_left_out() {
    let piece = c_minor();
    let heard = piece.heard();
    let most = MOST_BEATS as i64 * BEAT;
    for start in [i64::MIN / 2, -1_000_000 * BEAT, -most, most + 7 * BEAT, i64::MAX / 2] {
        let mut damaged = piece.clone();
        damaged.harmony.push(Note { start, length: BEAT, key: 61, velocity: LOUD });
        assert_eq!(damaged.heard(), heard, "a note at {start}");
        let mut reader = KeyReader::new();
        let (first_bar, bars) = reader.hear_bars(&damaged.parts(), Grid::four_four(TPB)).unwrap();
        let mut sum = Heard::default();
        bars.iter().for_each(|b| sum.merge(b));
        assert_eq!((first_bar, sum), (0, heard.clone()), "a note at {start}, bar by bar");
    }
    // A note far away but within the hearing is heard.
    let mut far = piece.clone();
    far.harmony.push(Note { start: most / 2, length: BEAT, key: 61, velocity: LOUD });
    assert_eq!(far.heard().notes, heard.notes + 1);
}

// Calibration and chances.

#[test]
fn the_calibration_is_a_curve_with_no_jumps() {
    let terms = |bars: u32| Calibration::terms(bars, Config::Full, Span::Window);
    let knot = |k: usize| std::array::from_fn::<f64, CALIBRATION_TERMS, _>(|i| if i == k { 1.0 } else { 0.0 });
    assert_eq!(terms(1), knot(0));
    assert_eq!(terms(0), terms(1));
    assert_eq!(terms(8), knot(3));
    assert_eq!(terms(16), knot(KNOTS - 1));
    assert_eq!(terms(1000), terms(16));
    let three = terms(3);
    assert!((three[1] - (2.0 - 3f64.log2())).abs() < 1e-15 && (three[2] - (3f64.log2() - 1.0)).abs() < 1e-15);
    // After the curve: melody only, bass only, accompaniment; then what a loop adds, on its own curve; then a song.
    let after = |bars: u32, config: Config, span: Span| Calibration::terms(bars, config, span)[KNOTS..].to_vec();
    assert_eq!(after(4, Config::BassOnly, Span::Song), [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    assert_eq!(after(4, Config::MelodyOnly, Span::Loop), [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    assert_eq!(after(64, Config::Accompaniment, Span::Loop), [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    assert_eq!(after(4, Config::Full, Span::Window), [0.0; 9]);
    let loop3 = after(3, Config::Full, Span::Loop);
    assert_eq!(loop3[4..6], three[1..3]);

    // Each bar more moves the sharpness a little, never in a jump, and it never drops below the least.
    for span in Span::ALL {
        let mut last = ODD_CALIBRATION.sharpness(1, Config::Full, span);
        for bars in 2..200 {
            let s = ODD_CALIBRATION.sharpness(bars, Config::Full, span);
            assert!((s - last).abs() < 0.3 && s >= LEAST_SHARPNESS, "{span:?} at {bars} bars");
            assert!(span != Span::Window || s >= last);
            last = s;
        }
        assert_eq!(last, ODD_CALIBRATION.sharpness(16, Config::Full, span), "flat past 16 bars");
    }
    let cold = Calibration([-1.0; CALIBRATION_TERMS]);
    assert_eq!(cold.sharpness(5, Config::Accompaniment, Span::Loop), LEAST_SHARPNESS);
    let broken = Calibration([f64::NAN; CALIBRATION_TERMS]);
    assert_eq!(broken.sharpness(5, Config::Full, Span::Song), LEAST_SHARPNESS);
}

#[test]
fn chances_add_up_and_turn_with_the_keys() {
    let z: [f64; 24] = std::array::from_fn(|i| ((i * 37 % 24) as f64 - 11.0) * 0.731);
    let p = chances(z);
    assert!((p.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    // The same numbers moved round the keys give the same chances moved round, to the last bit.
    for s in 1..12 {
        let turned: [f64; 24] = std::array::from_fn(|i| z[(i / 12) * 12 + (i % 12 + 12 - s) % 12]);
        let q = chances(turned);
        for i in 0..24 {
            assert_eq!(q[(i / 12) * 12 + (i % 12 + s) % 12].to_bits(), p[i].to_bits());
        }
    }
    assert_eq!(chances([f64::NAN; 24]), [1.0 / 24.0; 24]);
    let mut wild = [0.0; 24];
    wild[3] = f64::INFINITY;
    wild[4] = f64::NEG_INFINITY;
    assert!(chances(wild).iter().all(|p| p.is_finite()));
}
