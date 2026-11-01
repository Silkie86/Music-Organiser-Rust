//! Tests of the hearing step ([`Hearing`]): the units and their levels in each metre, the snap, the streams and the
//! bass found by the gaps, broken chords, notes held over, loops, the rules that hold for any notes (moved, shuffled,
//! doubled, at another resolution), damaged notes, and a slow way of working out each unit that the fast one must
//! agree with.

use std::ops::Range;

use super::{BASS_GAP, BassTexture, Hearing, Level, MOST_UNITS, Mass, Stream, Unit};
use crate::keyfind::Span;
use crate::notes::{Grid, Note, Part, Role};
use crate::pitch::{PcSet, PerPitch, PitchClass};

const PPQ: i64 = 480;
pub(super) const BEAT: i64 = PPQ;
pub(super) const BAR: i64 = 4 * PPQ;
pub(super) const VELOCITY: u16 = 1 << 15;

/// The MIDI key of a note's name, C4 being 60: "C4", "Eb3", "F#2".
pub(super) fn key(name: &str) -> u8 {
    let mut chars = name.chars();
    let mut semitones = match chars.next() {
        Some('C') => 0,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => panic!("not a note: {name}"),
    };
    let mut rest = chars.as_str();
    while let Some(r) = rest.strip_prefix('#') {
        semitones += 1;
        rest = r;
    }
    while let Some(r) = rest.strip_prefix('b') {
        semitones -= 1;
        rest = r;
    }
    let octave: i32 = rest.parse().unwrap();
    u8::try_from((octave + 1) * 12 + semitones).unwrap()
}

/// The notes named, all struck at `start` and held `length` ticks.
pub(super) fn notes(names: &str, start: i64, length: i64) -> Vec<Note> {
    names.split_whitespace().map(|n| Note { start, length, key: key(n), velocity: VELOCITY }).collect()
}

/// The notes named, one after another from `start`, each `step` ticks long.
pub(super) fn line(names: &str, start: i64, step: i64) -> Vec<Note> {
    let named = names.split_whitespace().enumerate();
    named.map(|(i, n)| Note { start: start + i as i64 * step, length: step, key: key(n), velocity: VELOCITY }).collect()
}

/// Pitch classes by name: "C E G".
pub(super) fn pcs(names: &str) -> PcSet {
    names.split_whitespace().map(|n| PitchClass::of_key(key(&format!("{n}4")))).collect()
}

/// Ticks for some pitch classes, none for the rest.
fn per_pitch(values: &[(PitchClass, u64)]) -> PerPitch<u64> {
    let mut out = PerPitch::default();
    for &(pc, ticks) in values {
        out[pc] = ticks;
    }
    out
}

/// Some music: parts with their roles, on a grid, heard as a loop, a song or a stretch.
#[derive(Clone, Debug)]
pub(super) struct Music {
    pub(super) parts: Vec<(Role, Vec<Note>)>,
    pub(super) grid: Grid,
    pub(super) span: Span,
}

impl Music {
    /// 4/4 at 480 ticks a beat, heard as a song.
    pub(super) fn new() -> Music {
        Music { parts: Vec::new(), grid: Grid::four_four(480), span: Span::Song }
    }

    pub(super) fn grid(mut self, beats_per_bar: u32, ticks_per_beat: u32) -> Music {
        self.grid = Grid { ticks_per_beat, beats_per_bar, bar_line: 0 };
        self
    }

    pub(super) fn looped(mut self) -> Music {
        self.span = Span::Loop;
        self
    }

    pub(super) fn part(mut self, role: Role, notes: Vec<Note>) -> Music {
        self.parts.push((role, notes));
        self
    }

    pub(super) fn bass(self, notes: Vec<Note>) -> Music {
        self.part(Role::Bass, notes)
    }

    pub(super) fn keys(self, notes: Vec<Note>) -> Music {
        self.part(Role::Harmony, notes)
    }

    pub(super) fn tune(self, notes: Vec<Note>) -> Music {
        self.part(Role::Melody, notes)
    }

    pub(super) fn hear_into(&self, hearing: &mut Hearing) -> bool {
        let parts: Vec<Part> = self.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect();
        hearing.hear(&parts, self.grid, self.span)
    }

    pub(super) fn hear(&self) -> Hearing {
        let mut hearing = Hearing::new();
        assert!(self.hear_into(&mut hearing), "nothing heard");
        hearing
    }

    /// The same music with every note changed.
    pub(super) fn map(&self, change: impl Fn(Note) -> Note) -> Music {
        let parts = self.parts.iter().map(|(role, notes)| (*role, notes.iter().map(|n| change(*n)).collect()));
        Music { parts: parts.collect(), ..self.clone() }
    }
}

/// The level weights of some units: "4 1 2 1 3 1 2 1" for a bar of 4/4.
fn levels(h: &Hearing, units: Range<usize>) -> String {
    units.map(|u| h.level(u).weight().to_string()).collect::<Vec<_>>().join(" ")
}

#[test]
fn units_are_half_beats_in_simple_metre_and_beats_in_compound() {
    // Beats in a bar, ticks in a beat, the unit, units in a felt beat, and the levels of a bar's units.
    let metres = [
        (4, 480, Unit::HalfBeat, 2, "4 1 2 1 3 1 2 1"),
        (3, 480, Unit::HalfBeat, 2, "4 1 2 1 2 1"),
        (2, 480, Unit::HalfBeat, 2, "4 1 3 1"),
        (5, 480, Unit::HalfBeat, 2, "4 1 2 1 2 1 2 1 2 1"),
        (7, 240, Unit::HalfBeat, 2, "4 1 2 1 2 1 2 1 2 1 2 1 2 1"),
        (6, 240, Unit::Beat, 3, "4 1 1 3 1 1"),
        (9, 240, Unit::Beat, 3, "4 1 1 2 1 1 2 1 1"),
        (12, 240, Unit::Beat, 3, "4 1 1 2 1 1 3 1 1 2 1 1"),
        // 6/4 is heard as compound too (the design's open question 7).
        (6, 480, Unit::Beat, 3, "4 1 1 3 1 1"),
        // A grid of one tick a beat can't be halved.
        (4, 1, Unit::Beat, 1, "4 2 3 2"),
    ];
    for (beats, tpb, unit, felt, want) in metres {
        let bar = i64::from(beats) * i64::from(tpb);
        let h = Music::new().grid(beats, tpb).keys(notes("C4 E4 G4", 0, 2 * bar)).hear();
        let per_bar = want.split(' ').count();
        assert_eq!((h.unit(), h.felt_units(), h.bar_units()), (unit, felt, per_bar), "{beats} beats of {tpb}");
        assert_eq!(h.units(), 2 * per_bar);
        assert_eq!(levels(&h, 0..per_bar), want);
        assert_eq!(levels(&h, per_bar..2 * per_bar), want);
        assert_eq!((h.start(per_bar), h.start(2 * per_bar)), (bar, 2 * bar));
        let unit_ticks = if unit == Unit::Beat { i64::from(tpb) } else { i64::from(tpb) / 2 };
        assert_eq!(h.felt_ticks(), felt as i64 * unit_ticks);
    }
    // An odd number of ticks a beat: unit k starts ⌊3k/2⌋ ticks in.
    let h = Music::new().grid(4, 3).keys(notes("C4", 0, 12)).hear();
    assert_eq!((0..=8).map(|u| h.start(u)).collect::<Vec<_>>(), [0, 1, 3, 4, 6, 7, 9, 10, 12]);
    // A damaged grid (no ticks a beat, no beats a bar) is heard as one tick a beat and one beat a bar.
    let h = Music::new().grid(0, 0).keys(notes("C4", 0, 3)).hear();
    assert_eq!((h.unit(), h.units(), levels(&h, 0..3)), (Unit::Beat, 3, "4 4 4".to_string()));
    // Units start on the grid's bar lines, wherever they are.
    let mut music = Music::new().keys(notes("C4", 100, BAR));
    music.grid.bar_line = 100;
    let h = music.hear();
    assert_eq!((h.start(0), h.start(1), h.level(0), h.level(4)), (100, 340, Level::Bar, Level::HalfBar));
    assert_eq!(h.tick(0), 100);
}

#[test]
fn a_note_struck_just_before_a_unit_starts_is_heard_on_it() {
    // A quarter of a 240-tick unit is 60 ticks: a note 60 ticks early is heard on the unit, one 61 ticks early isn't.
    for (at, unit, heard_at) in
        [(420, 2, 480), (419, 1, 419), (479, 2, 480), (480, 2, 480), (1890, 8, BAR), (1919, 8, BAR)]
    {
        let h = Music::new().keys(notes("C4", 0, 4 * BAR)).keys(notes("E4", at, 100)).hear();
        let struck: Vec<usize> =
            (0..h.units()).filter(|&u| h.struck(u, Stream::Harmony).contains(PitchClass::E)).collect();
        assert_eq!(struck, [unit], "struck at {at}");
        let e4 = h.notes().iter().find(|n| n.key == key("E4")).unwrap();
        assert_eq!(e4.start, heard_at, "struck at {at}");
        assert_eq!(e4.end, at + 100, "it still ends where it was let go");
    }
}

#[test]
fn a_mass_counts_the_longest_note_of_each_pitch_class_in_each_unit() {
    let h = Music::new().keys(notes("C4 E4 G4 C5", 0, BAR)).keys(notes("D4", 100, 200)).hear();
    let m = h.mass(0..8, Mass::Harmony);
    assert_eq!((m[PitchClass::C], m[PitchClass::E], m[PitchClass::G]), (1920, 1920, 1920), "C5 counts once");
    assert_eq!(h.mass(0..1, Mass::Harmony)[PitchClass::D], 140);
    assert_eq!(h.mass(1..2, Mass::Harmony)[PitchClass::D], 60);
    assert_eq!(m[PitchClass::D], 200);
    // Two notes of one pitch class in a unit count as the longer of them.
    let h = Music::new().keys(notes("C4", 0, 100)).keys(notes("C5", 100, 140)).keys(notes("E4", 0, BAR)).hear();
    assert_eq!(h.mass(0..1, Mass::Harmony)[PitchClass::C], 140);
    // A stretch past the end or backwards is cut to what's there.
    let (from, to) = (5, 3);
    assert_eq!(h.mass(from..to, Mass::Harmony), PerPitch::default());
    assert_eq!(h.mass(0..1000, Mass::Harmony), h.mass(0..h.units(), Mass::Harmony));
}

#[test]
fn a_note_is_heard_when_it_sounds_an_eighth_of_the_time() {
    // Over a bar of C E G (1920 ticks sounding), a D sounding 240 ticks is half present, so heard; 239 isn't.
    for (length, heard) in [(240, true), (239, false)] {
        let h = Music::new().keys(notes("C4 E4 G4", 0, BAR)).keys(notes("D5", 0, length)).hear();
        assert_eq!(h.sounding_ticks(0..8), 1920);
        assert_eq!(h.heard(0..8), if heard { pcs("C D E G") } else { pcs("C E G") });
        assert!((h.presence(0..8)[PitchClass::D] - length as f64 / 480.0).abs() < 1e-12);
        assert_eq!(h.presence(0..8)[PitchClass::C], 1.0);
    }
}

#[test]
fn the_tune_counts_half_and_only_its_notes_held_a_felt_beat() {
    // A held A over a bar of C E G: held a beat, it's 4 x 1/2 x 480 / 1920 = 1/2 present. A tick shorter, it's a quick
    // note, and the tune's quick notes don't count.
    for (length, presence) in [(480, 0.5), (479, 0.0), (960, 1.0)] {
        let h = Music::new().keys(notes("C4 E4 G4", 0, BAR)).tune(notes("A5", 0, length)).hear();
        assert_eq!(h.presence(0..8)[PitchClass::A], presence, "{length}");
        assert_eq!(h.mass(0..8, Mass::Melody)[PitchClass::A], length as u64);
    }
    // A tune alone: nothing sounds in the chords or the bass, so its held notes are all there is.
    let h = Music::new().tune(notes("A5", 0, 960)).tune(notes("B5", 960, 240)).hear();
    assert_eq!(h.sounding_ticks(0..h.units()), 0);
    assert_eq!(h.heard(0..h.units()), pcs("A"));
}

#[test]
fn presence_counts_chords_and_bass_twice_and_held_tune_notes_once() {
    let h = Music::new().bass(notes("C2", 0, BAR)).keys(notes("E4 G4", 0, BEAT)).tune(notes("A5", 0, 960)).hear();
    let ticks = h.presence_ticks(0..8);
    assert_eq!((ticks[PitchClass::C], ticks[PitchClass::E], ticks[PitchClass::A]), (3840, 960, 960));
    assert_eq!(h.presence(0..8)[PitchClass::E], 1.0, "a beat of a bar is a quarter: fully present");
    assert_eq!(h.presence(0..8)[PitchClass::A], 1.0);
}

#[test]
fn the_lowest_note_is_the_lowest_chord_or_bass_note_sounding() {
    let h = Music::new()
        .bass(notes("C2", 0, BEAT))
        .keys(notes("E3 G3", 0, BAR + 100))
        .keys(notes("D3", 2 * BEAT, 50))
        .keys(notes("B3 D4", BAR, BAR))
        .tune(notes("A1", BEAT, BEAT))
        .hear();
    assert_eq!(h.lowest(0..1), Some(key("C2")));
    // The tune isn't a chord note, however low.
    assert_eq!(h.lowest(2..4), Some(key("E3")));
    // A note sounding less than a quarter of the unit doesn't count, nor does a held-over end.
    assert_eq!(h.lowest(4..5), Some(key("E3")));
    assert_eq!(h.lowest(8..9), Some(key("B3")));
    assert_eq!(h.lowest(0..8), Some(key("C2")));
    let (from, to) = (5, 3);
    assert_eq!((h.lowest(16..17), h.lowest(from..to)), (None, None));
    // Where no chord or bass note sounds a quarter of a unit, the lowest note sounding at all, of any stream: a tune
    // alone, or chords too short.
    let h =
        Music::new().tune(notes("E5", 0, BAR)).keys(notes("G4 C5", BAR, 50)).keys(notes("A3", BAR + 120, 40)).hear();
    assert_eq!((h.lowest(0..8), h.lowest(8..9), h.lowest(0..9)), (Some(key("E5")), Some(key("A3")), Some(key("A3"))));
    assert_eq!(h.heard(0..8), pcs("E"));
}

#[test]
fn the_notes_heard_are_those_sounding_an_eighth_of_the_time() {
    let ticks = per_pitch(&[(PitchClass::C, 3840), (PitchClass::D, 480), (PitchClass::E, 479)]);
    assert_eq!(super::heard_in(&ticks, 1920), pcs("C D"));
    // Nothing sounding: any tick is heard (a tune alone).
    assert_eq!(super::heard_in(&per_pitch(&[(PitchClass::A, 1)]), 0), pcs("A"));
    assert_eq!(super::heard_in(&PerPitch::default(), 0), PcSet::EMPTY);
}

#[test]
fn a_tick_lines_up_with_the_nearest_unit_start() {
    let h = Music::new().keys(notes("C4 E4 G4", 0, 2 * BAR)).hear();
    assert_eq!(h.units(), 16);
    let cases = [(0, 0), (119, 0), (120, 0), (121, 1), (240, 1), (-500, 0), (2 * BAR - 121, 15), (2 * BAR - 120, 15)];
    for (tick, unit) in cases.into_iter().chain([(2 * BAR - 119, 16), (2 * BAR, 16), (i64::MAX, 16), (i64::MIN, 0)]) {
        assert_eq!(h.nearest_unit(tick), unit, "{tick}");
    }
    // The units start on the music's own bar line.
    let moved = Music::new().grid(4, 480);
    let mut moved = moved.keys(notes("C4 E4 G4", 1000, BAR));
    moved.grid.bar_line = 1000;
    let h = moved.hear();
    assert_eq!((h.nearest_unit(1000), h.nearest_unit(1000 + 240), h.nearest_unit(1119)), (0, 1, 0));
    // Three ticks a beat: units of one tick and two, starting at 0, 1, 3, 4, 6...; halfway goes to the earlier.
    let h = Music::new().grid(4, 3).keys(notes("C4 E4 G4", 0, 12)).hear();
    assert_eq!((1..=7).map(|t| h.nearest_unit(t)).collect::<Vec<_>>(), [1, 1, 2, 3, 3, 4, 5]);
    // A loop's units start with its copied last bar: its own first unit comes after it.
    let h = Music::new().looped().keys(notes("C4 E4 G4", 0, BAR)).hear();
    assert_eq!((h.nearest_unit(0), h.nearest_unit(-BAR)), (h.bar_units(), 0));
}

#[test]
fn silence_inside_a_stretch_is_free() {
    let h = Music::new().keys(notes("C4 E4 G4", 0, BEAT)).bass(notes("C2", 3 * BEAT, BEAT)).hear();
    assert_eq!(h.sounding_ticks(0..8), 960);
    assert_eq!(h.presence(0..8)[PitchClass::E], 1.0);
}

#[test]
fn a_bass_part_is_the_bass() {
    let h = Music::new().bass(notes("C2", 0, BAR)).keys(notes("C3 E4 G4", 0, BAR)).hear();
    assert!(h.has_bass_part());
    assert_eq!(h.mass(0..8, Mass::Bass), per_pitch(&[(PitchClass::C, 1920)]));
    assert_eq!(h.mass(0..8, Mass::Harmony)[PitchClass::C], 1920, "the keys' own C stays in the chords");
    assert_eq!(h.bass(0..8), BassTexture::Held(PitchClass::C));
    assert_eq!((h.bass_onset(0..8), h.bass_onset(1..8)), (Some(PitchClass::C), None));
    assert!((0..8).all(|u| h.gap_bass(u).is_none()), "no bass from the gaps beside a bass part");
    assert_eq!((h.struck(0, Stream::Bass), h.carried(1, Stream::Bass)), (pcs("C"), pcs("C")));
    // Two bass notes struck together: the lower is the onset.
    let h = Music::new().bass(notes("G2 C2", 0, BAR)).keys(notes("C4 E4 G4", 0, BAR)).hear();
    assert_eq!(h.bass_onset(0..8), Some(PitchClass::C));
}

#[test]
fn with_no_bass_part_the_bass_is_found_by_the_gaps() {
    // Keys, and the bass found: the design's cases 13, 14, 34, 49 and 50, chords in fourths, lone notes, a tritone.
    let cases = [
        ("B2 E3 F3 A3", None),
        ("D3 C4 F4", Some("D3")),
        ("G2 B3 F4", Some("G2")),
        ("C3 G3 D4 Eb4 Bb4 F5", Some("C3")),
        ("C2 G2 C3 E4 G4 B4 D5", Some("C2")),
        ("E2 E3 C4 G4 B4 D5", Some("E2")),
        ("D3 G3 C4", None),
        ("C3 F#3 C4", None),
        ("C3", None),
        ("C3 C3", None),
        ("C3 C4", None),
        ("C2 C3 C4", None),
        ("C2 C3 G3", Some("C2")),
    ];
    for (keys, bass) in cases {
        let h = Music::new().keys(notes(keys, 0, BAR)).hear();
        let want = bass.map(key);
        assert!((0..8).all(|u| h.gap_bass(u) == want), "{keys}");
        let texture = want.map_or(BassTexture::None, |k| BassTexture::Held(PitchClass::of_key(k)));
        assert_eq!(h.bass(0..8), texture, "{keys}");
    }
    assert_eq!(BASS_GAP, 7);
    // A lone note and its octave double are heard the same: the double is the note doubled, not a chord over it.
    let lone = |keys: &str| unit_facts(&Music::new().keys(notes(keys, 0, BAR)).hear(), 0, 1);
    assert_eq!(lone("C3 C4"), lone("C3"));
    // With only its octave above it, a note is the bass if it was just before: a held bass stays the bass.
    let h = Music::new().keys(notes("D3", 0, BAR)).keys(notes("C4 F4", 0, BEAT)).keys(notes("D4", BEAT, BEAT)).hear();
    assert!((0..8).all(|u| h.gap_bass(u) == Some(key("D3"))));
    let h = Music::new().keys(notes("D3", 0, BAR)).keys(notes("D4", 0, BEAT)).keys(notes("C4 F4", BEAT, BEAT)).hear();
    let gaps: Vec<Option<u8>> = (0..8).map(|u| h.gap_bass(u)).collect();
    assert_eq!(gaps, [&[None; 2][..], &[Some(key("D3")); 6]].concat());
    // The gap bass's ticks are the bass's, not the chords'; so are its octave doubles' (the bass, doubled).
    let h = Music::new().keys(notes("D3 C4 F4", 0, BAR)).hear();
    assert_eq!((h.mass(0..8, Mass::Harmony)[PitchClass::D], h.mass(0..8, Mass::Bass)[PitchClass::D]), (0, 1920));
    assert_eq!((h.struck(0, Stream::Bass), h.struck(0, Stream::Harmony)), (pcs("D"), pcs("C F")));
    assert_eq!(h.bass_onset(0..8), Some(PitchClass::D));
    let h = Music::new().keys(notes("C2 C3 E4 G4 C5", 0, BAR)).hear();
    assert_eq!(h.gap_bass(0), Some(key("C2")));
    assert_eq!((h.mass(0..8, Mass::Harmony)[PitchClass::C], h.mass(0..8, Mass::Bass)[PitchClass::C]), (0, 1920));
    assert_eq!((h.sounding(0, Stream::Harmony), h.sounding(0, Stream::Bass)), (pcs("E G"), pcs("C")));
    // With a bass part, the keys' lowest note is never the bass (the design's case 11).
    let h = Music::new().bass(notes("D2", 0, BAR)).keys(notes("C3 E3 F3 A3", 0, BAR)).hear();
    assert!((0..8).all(|u| h.gap_bass(u).is_none()));
    assert_eq!(h.mass(0..8, Mass::Bass), per_pitch(&[(PitchClass::D, 1920)]));
}

#[test]
fn the_bass_is_found_by_the_gaps_in_bars_where_no_bass_part_plays() {
    // The left hand's D3 under C4 F4 for four bars, and a bass part that only plays bar 3: D3 is the bass in bars 1, 2
    // and 4, and the bass part's G2 in bar 3, where the D3 stays in the chords.
    let keys: Vec<Note> = (0..4).flat_map(|b| notes("D3 C4 F4", b * BAR, BAR)).collect();
    let music = Music::new().keys(keys).bass(notes("G2", 2 * BAR, BAR));
    let h = music.hear();
    let gaps: Vec<Option<u8>> = (0..32).map(|u| h.gap_bass(u)).collect();
    let want: Vec<Option<u8>> = (0..32).map(|u| (!(16..24).contains(&u)).then_some(key("D3"))).collect();
    assert_eq!(gaps, want);
    assert!(h.has_bass_part());
    let (d, g) = (BassTexture::Held(PitchClass::D), BassTexture::Held(PitchClass::G));
    assert_eq!((h.bass(0..16), h.bass(16..24), h.bass(24..32)), (d, g, d));
    assert_eq!((h.mass(16..24, Mass::Harmony)[PitchClass::D], h.mass(16..24, Mass::Bass)[PitchClass::D]), (1920, 0));
    // The last moment of a bass note played legato into bar 4 doesn't make the bass part play there; a short note
    // struck there does.
    let tail = music.clone().bass(notes("A2", 3 * BAR - 100, 110)).hear();
    assert!((24..32).all(|u| tail.gap_bass(u) == Some(key("D3"))));
    let hit = music.bass(notes("A2", 3 * BAR + 3 * BEAT, 20)).hear();
    assert!((24..32).all(|u| hit.gap_bass(u).is_none()));
    assert_eq!(hit.bass(24..32), BassTexture::Held(PitchClass::A));
}

#[test]
fn the_last_moment_of_a_note_played_legato_is_no_bass() {
    // The left hand's C3 rings 5 ticks into beat 2, where D3 is struck: D3 is that unit's bass, as the C3 sounds for
    // less than a quarter of it.
    let h =
        Music::new().keys(notes("C3", 0, BEAT + 5)).keys(notes("E4 G4", 0, BEAT)).keys(notes("D3 F4 A4", BEAT, BEAT));
    let h = h.hear();
    assert_eq!((h.gap_bass(1), h.gap_bass(2)), (Some(key("C3")), Some(key("D3"))));
    assert_eq!((h.sounding(2, Stream::Harmony), h.sounding(2, Stream::Bass)), (pcs("F A"), pcs("D")));
    assert_eq!(h.mass(2..3, Mass::Harmony)[PitchClass::C], 5, "its ticks still count");
}

#[test]
fn an_arpeggio_is_heard_as_its_chord_and_is_never_the_bass() {
    // The design's case 40: eighths, C3 E3 G3 C4 E4 C4 G3 E3 | F3 A3 C4 F4 A4 F4 C4 A3.
    let h = Music::new().keys(line("C3 E3 G3 C4 E4 C4 G3 E3 F3 A3 C4 F4 A4 F4 C4 A3", 0, BEAT / 2)).hear();
    assert_eq!((h.heard(0..8), h.heard(8..16)), (pcs("C E G"), pcs("F A C")));
    assert_eq!(h.bass(0..16), BassTexture::None);
    // Each felt beat's first note is held by the pedal to the end of the beat; the second ends there anyway. Both are
    // broken, as both are struck as part of the broken chord.
    let first = h.notes().iter().find(|n| n.start == 0).unwrap();
    assert!(first.broken && first.end == BEAT && first.length == BEAT / 2);
    let second = h.notes().iter().find(|n| n.start == BEAT / 2).unwrap();
    assert!(second.broken && second.end == BEAT);
    assert_eq!((h.broken_struck(0), h.broken_struck(1)), (pcs("C"), pcs("E")));
    assert_eq!(h.sounding(1, Stream::Harmony), pcs("C E"));
}

#[test]
fn a_bass_held_through_each_beat_under_an_arpeggio_is_the_bass() {
    // The left hand's C3 a quarter note on each beat, with C4 E4 in eighths above it: each C3 is held through its
    // beat, so it isn't broken and is the bass; the C4s and E4s are broken.
    let keys: Vec<Note> =
        (0..4).flat_map(|b| [notes("C3", b * BEAT, BEAT), line("C4 E4", b * BEAT, BEAT / 2)].concat()).collect();
    let h = Music::new().keys(keys.clone()).hear();
    assert!(h.notes().iter().all(|n| n.broken == (n.key != key("C3"))));
    assert_eq!(h.bass(0..8), BassTexture::Held(PitchClass::C));
    // Each C3 let go of 10 ticks early (a player's timing), the same.
    let early: Vec<Note> =
        keys.iter().map(|&n| if n.key == key("C3") { Note { length: n.length - 10, ..n } } else { n }).collect();
    let h = Music::new().keys(early).hear();
    assert!(h.notes().iter().all(|n| n.broken == (n.key != key("C3"))));
    assert_eq!(h.bass(0..8), BassTexture::Held(PitchClass::C));
    // In a loop, the last C3 held past the loop's end is cut there, and is still held through its beat.
    let mut held_on = keys;
    if let Some(last) = held_on.iter_mut().rfind(|n| n.key == key("C3")) {
        last.length += 100;
    }
    let h = Music::new().looped().keys(held_on).hear();
    assert_eq!(h.reported(), 8..16);
    assert!(h.notes().iter().all(|n| n.broken == (n.key != key("C3"))));
    assert_eq!(h.bass(8..16), BassTexture::Held(PitchClass::C));
}

#[test]
fn an_alberti_bass_is_heard_as_its_chord() {
    // The design's case 42, in eighths and in sixteenths: C3 G3 E3 G3 over and over.
    for step in [BEAT / 2, BEAT / 4] {
        let names = "C3 G3 E3 G3 ".repeat((BAR / step / 4) as usize);
        let h = Music::new().keys(line(&names, 0, step)).hear();
        assert_eq!(h.heard(0..8), pcs("C E G"), "{step}");
        assert_eq!(h.bass(0..8), BassTexture::None, "{step}");
    }
}

#[test]
fn chords_repeated_in_eighths_are_not_broken() {
    // The left hand's C2 with the chord, every eighth: no note is broken, and the C2 is the bass all through.
    let keys: Vec<Note> = (0..8).flat_map(|i| notes("C2 C4 E4 G4", i * BEAT / 2, BEAT / 2)).collect();
    let h = Music::new().keys(keys).hear();
    assert!(h.notes().iter().all(|n| !n.broken));
    assert!((0..8).all(|u| h.gap_bass(u) == Some(key("C2"))));
    assert_eq!(h.bass(0..8), BassTexture::Held(PitchClass::C));
    // Played a little unevenly (each chord's notes 3 ticks apart), the same.
    let mut keys = Vec::new();
    for i in 0..8 {
        for (j, name) in ["C2", "C4", "E4", "G4"].into_iter().enumerate() {
            let start = i * BEAT / 2 + 3 * j as i64;
            keys.push(Note { start, length: BEAT / 2 - 10, key: key(name), velocity: VELOCITY });
        }
    }
    let h = Music::new().keys(keys).hear();
    assert!(h.notes().iter().all(|n| !n.broken));
    assert!((0..8).all(|u| h.gap_bass(u) == Some(key("C2"))));
}

#[test]
fn a_note_is_the_bass_where_the_masses_count_it() {
    let arpeggio = [notes("C2", 0, BAR), line("E4 G4 C5 G4 E4 G4 C5 G4", 0, BEAT / 2)].concat();
    let musics = [
        // A held C2 under eighths with C5 among them (broken, so the chords'), and a tune's C6 (never the bass).
        Music::new().keys(arpeggio).tune(notes("C6", 0, BAR)),
        // The left hand's C2 and its octave C3 under a chord, held into the next bar a little: the held-over end isn't
        // the bass.
        Music::new()
            .keys([notes("C2 C3 E4 G4", 0, BAR + BEAT / 2), notes("F2 F3 A4 C5", BAR + BEAT / 2, BAR)].concat()),
        // A bass part: only its notes.
        Music::new().bass(notes("C2", 0, BAR)).keys(notes("C3 E4 G4", 0, BAR)),
    ];
    for music in musics {
        let h = music.hear();
        assert!(h.notes().iter().any(|n| (0..h.units()).any(|u| h.is_bass_in(u, n))));
        // A broken chord's notes are the chords', on the bass's pitch class too.
        assert!(h.notes().iter().filter(|n| n.broken).all(|n| (0..h.units()).all(|u| !h.is_bass_in(u, n))));
        for u in 0..h.units() {
            let (from, to) = (h.offset(u), h.offset(u + 1));
            // The longest any one of them sounds in the unit (octave doubles count once), held-over ends left out.
            let mut bass = PerPitch::<u64>::default();
            for n in h.notes().iter().filter(|n| h.is_bass_in(u, n)) {
                let end = n.held_over.unwrap_or(n.end).clamp(n.start, n.end);
                let ticks = u64::try_from(end.min(to) - n.start.max(from)).unwrap_or(0);
                bass[n.pitch_class()] = bass[n.pitch_class()].max(ticks);
            }
            assert_eq!(bass, h.mass(u..u + 1, Mass::Bass), "unit {u}");
        }
    }
}

#[test]
fn a_held_note_under_an_arpeggio_is_not_broken() {
    // One part: a held C2, and eighths over it. The eighths are broken; the C2 isn't, and it's the bass.
    let keys = [notes("C2", 0, BAR), line("E4 G4 C5 G4 E4 G4 C5 G4", 0, BEAT / 2)].concat();
    let h = Music::new().keys(keys).hear();
    let c2 = h.notes().iter().find(|n| n.key == key("C2")).unwrap();
    assert!(!c2.broken);
    assert!(h.notes().iter().any(|n| n.broken));
    assert!((0..8).all(|u| h.gap_bass(u) == Some(key("C2"))));
    // An arpeggio written with the pedal in its notes (each held to the end of the beat) overlaps by more than a unit:
    // nothing is left for the pedal to hold.
    let held = ["E4", "G4", "C5"].into_iter().enumerate();
    let keys =
        held.map(|(i, k)| Note { start: i as i64 * 120, length: BEAT - i as i64 * 120, key: key(k), velocity: 1 });
    let h = Music::new().keys(keys.collect()).hear();
    assert!(h.notes().iter().all(|n| !n.broken));
}

#[test]
fn a_chord_held_to_the_end_of_the_music_is_held_through_its_felt_beat() {
    // The left hand's C3 and G3 by turns under the right hand's E4 G4, in one part. The music ends 260 ticks into the
    // felt beat: E4 G4 held to the end count as held through it, so the felt beat isn't broken...
    let turns = [notes("C3", 0, 100), notes("G3", BEAT / 2, 20)].concat();
    let h = Music::new().keys([turns.clone(), notes("E4 G4", 0, 260)].concat()).hear();
    assert!(h.notes().iter().all(|n| !n.broken));
    // ...as with E4 G4 held to the felt beat's end and more music after it.
    let h = Music::new().keys([turns, notes("E4 G4", 0, BEAT), notes("F4", BAR, BEAT)].concat()).hear();
    assert!(h.notes().iter().all(|n| !n.broken));
}

#[test]
fn a_left_hand_playing_bass_and_chord_by_turns_on_its_own_is_broken() {
    // C2 on each beat, E3 G3 between: alone, it's heard as a broken chord, as the design has it, so the C2 is no bass.
    // With the right hand's held chord in the same part, it isn't broken, and the C2 is the bass on each beat; so too
    // when the chord is let go of 10 ticks before the bar's end (a player's timing), as it's still held through the
    // last beat.
    let beat = |b: i64| [notes("C2", b * BEAT, BEAT / 2), notes("E3 G3", b * BEAT + BEAT / 2, BEAT / 2)].concat();
    let left: Vec<Note> = (0..4).flat_map(beat).collect();
    let h = Music::new().keys(left.clone()).hear();
    assert!((0..8).all(|u| h.gap_bass(u).is_none()));
    for held in [BAR, BAR - 10] {
        let h = Music::new().keys([left.clone(), notes("C4 E4 G4", 0, held)].concat()).hear();
        assert!((0..8).step_by(2).all(|u| h.gap_bass(u) == Some(key("C2"))), "{held}");
        assert_eq!(h.bass(0..8), BassTexture::Held(PitchClass::C), "{held}");
    }
}

#[test]
fn a_chord_held_a_little_past_the_bar_line_is_kept_apart() {
    // The design's case 51: Cmaj7 from 0 to 2160, Fmaj7 from 1920. The B rings an eighth into bar 2: held over.
    let h = Music::new()
        .bass(line("C2 F2", 0, BAR))
        .keys(notes("C4 E4 G4 B4", 0, BAR + BEAT / 2))
        .keys(notes("F4 A4 C5 E5", BAR, BAR))
        .hear();
    assert_eq!((h.mass(8..16, Mass::Harmony)[PitchClass::B], h.mass(8..16, Mass::HeldOver)[PitchClass::B]), (0, 240));
    assert_eq!(h.heard(8..16), pcs("F A C E"));
    assert!(h.carried(8, Stream::Harmony).contains(PitchClass::B));
    assert!(!h.sounding(8, Stream::Harmony).contains(PitchClass::B) && !h.struck(8, Stream::Harmony).is_empty());
    let b4 = h.notes().iter().find(|n| n.key == key("B4")).unwrap();
    assert_eq!(b4.held_over, Some(BAR));
    // The design's case 52: C E G held to 2400, a whole felt beat into bar 2, is still held over; a tick longer isn't.
    for (end, held) in [(BAR + BEAT, true), (BAR + BEAT + 1, false)] {
        let h = Music::new().keys(notes("C4 E4 G4", 0, end)).keys(notes("F3 A3 C4", BAR, BAR)).hear();
        assert_eq!(h.mass(8..16, Mass::Harmony)[PitchClass::E] == 0, held, "{end}");
    }
    // A note struck less than a felt beat before the bar line (an eighth early: a push) is never held over; one
    // struck a beat before is.
    for (start, held) in [(BAR - BEAT / 2, false), (BAR - BEAT, true)] {
        let h = Music::new().keys(notes("C4", start, BAR + 100 - start)).hear();
        assert_eq!(h.notes()[0].held_over.is_some(), held, "{start}");
    }
    // The middle of a bar counts too, and a note held for bars is held over only past its last bar line.
    let h = Music::new().keys(notes("C4", 0, 2 * BEAT + 10)).keys(notes("E4", 0, 3 * BAR + 10)).hear();
    let held: Vec<_> = h.notes().iter().map(|n| (n.key, n.held_over)).collect();
    assert_eq!(held, [(key("C4"), Some(2 * BEAT)), (key("E4"), Some(3 * BAR))]);
}

#[test]
fn each_unit_knows_what_is_struck_in_it_and_what_is_carried_in() {
    let h = Music::new().keys(notes("C4 E4 G4", 0, 2 * BEAT)).keys(notes("B4", BEAT / 2, BEAT)).hear();
    assert_eq!((h.struck(0, Stream::Harmony), h.carried(0, Stream::Harmony)), (pcs("C E G"), PcSet::EMPTY));
    assert_eq!((h.struck(1, Stream::Harmony), h.carried(1, Stream::Harmony)), (pcs("B"), pcs("C E G")));
    assert_eq!((h.struck(2, Stream::Harmony), h.carried(2, Stream::Harmony)), (PcSet::EMPTY, pcs("C E G B")));
    assert_eq!(
        (h.struck_in(0..4, Stream::Harmony), h.struck_in(2..4, Stream::Harmony)),
        (pcs("C E G B"), PcSet::EMPTY)
    );
    assert_eq!(h.struck(0, Stream::Melody), PcSet::EMPTY);
}

#[test]
fn a_bass_is_held_when_one_note_has_three_quarters_of_its_time() {
    let texture = |bass: Vec<Note>| Music::new().bass(bass).keys(notes("C4 E4 G4", 0, BAR)).hear().bass(0..8);
    assert_eq!(texture(notes("C2", 0, BAR)), BassTexture::Held(PitchClass::C));
    assert_eq!(texture(line("C2 C3 C2 C3", 0, BEAT)), BassTexture::Held(PitchClass::C), "octave jumps");
    assert_eq!(texture(line("C2 C2 C2 G2", 0, BEAT)), BassTexture::Held(PitchClass::C), "three quarters");
    assert_eq!(texture([notes("C2", 0, 1200), notes("G2", 1200, 720)].concat()), BassTexture::Moving);
    assert_eq!(texture(line("D2 E2 F2 G2", 0, BEAT)), BassTexture::Moving);
    let h = Music::new().bass(line("D2 E2 F2 G2", 0, BEAT)).keys(notes("F3 A3 C4", 0, BAR)).hear();
    assert_eq!(
        (h.bass_onset(0..8), h.bass_onset(1..8), h.bass_onset(7..8)),
        (Some(PitchClass::D), Some(PitchClass::E), None)
    );
    assert_eq!(Music::new().keys(notes("C4 E4 G4", 0, BAR)).hear().bass(0..8), BassTexture::None);
}

#[test]
fn the_weighted_bass_counts_where_the_bass_notes_fall() {
    // C2 on beat 1, G2 on beat 2: C has 240 ticks at weight 4 and 240 at 1; G has 240 at 2 and 240 at 1.
    let h = Music::new().bass(line("C2 G2", 0, BEAT)).keys(notes("C4 E4 G4", 0, BAR)).hear();
    let w = h.mass(0..8, Mass::WeightedBass);
    assert_eq!((w[PitchClass::C], w[PitchClass::G]), (1200, 720));
}

#[test]
fn a_loop_is_heard_with_its_last_bar_copied_in_front() {
    let music = Music::new()
        .looped()
        .bass(line("C2 G2", 0, BAR))
        .keys(notes("C4 E4 G4", 0, BAR))
        .keys(notes("B3 D4 G4", BAR, BAR));
    let h = music.hear();
    assert_eq!((h.units(), h.reported()), (24, 8..24));
    assert_eq!((h.start(0), h.start(8)), (-BAR, 0));
    assert_eq!(h.mass(0..8, Mass::Bass), per_pitch(&[(PitchClass::G, 1920)]));
    assert_eq!((h.heard(0..8), h.heard(8..16), h.heard(16..24)), (pcs("G B D"), pcs("C E G"), pcs("G B D")));
    assert_eq!(h.notes().iter().filter(|n| n.copy).count(), 4);
    assert!(h.notes().iter().filter(|n| n.copy).all(|n| n.struck && n.start == 0));
    // A note played just before the loop's end (a quarter of a unit or less) is heard on its start, as the loop goes
    // round: the loop stays two bars long.
    let h = music.clone().keys(notes("C5", 2 * BAR - 10, 200)).hear();
    assert_eq!((h.units(), h.reported()), (24, 8..24));
    let c5: Vec<_> = h.notes().iter().filter(|n| n.key == key("C5")).map(|n| (n.start, n.copy, n.struck)).collect();
    assert_eq!(c5, [(BAR, false, true)]);
    assert!(h.struck(8, Stream::Harmony).contains(PitchClass::C) && !h.truncated());
    // Heard as a song, it has no copy.
    let h = Music { span: Span::Song, ..music }.hear();
    assert_eq!((h.units(), h.reported()), (16, 0..16));
    assert!(h.notes().iter().all(|n| !n.copy && !n.wraps));
    // A loop ends at the bar line after its last onset: a chord ringing a little past it is cut there, and wraps.
    let h = Music::new().looped().keys(notes("C4 E4 G4", 0, BAR + 10)).hear();
    assert_eq!(h.reported(), 8..16);
    assert!(h.notes().iter().all(|n| n.wraps));
    // A loop of two 2-bar chords: the F chord, struck at bar 3 and held to the end of bar 4 (or to within a felt beat
    // of it), takes the loop to 4 bars.
    for held in [2 * BAR, 2 * BAR - BEAT, 2 * BAR + BEAT / 2] {
        let keys = [notes("C4 E4 G4", 0, 2 * BAR), notes("F4 A4 C5", 2 * BAR, held)].concat();
        let h = Music::new().looped().keys(keys).hear();
        assert_eq!(h.reported(), 8..40, "{held}");
    }
    // Ringing on to the middle of bar 4, it's a release: the loop ends at bar 3's end, and the chord is cut there.
    let keys = [notes("C4 E4 G4", 0, 2 * BAR), notes("F4 A4 C5", 2 * BAR, BAR + BAR / 2)].concat();
    let h = Music::new().looped().keys(keys).hear();
    assert_eq!(h.reported(), 8..32);
}

#[test]
fn a_chord_struck_at_a_loops_end_and_held_to_it_wraps_round() {
    // The design's case 46: a 2-bar loop, silent to 960; G7 from 960 to 3600; Fmaj7 struck at 3600 and held to the
    // loop's end.
    let h = Music::new()
        .looped()
        .bass(notes("G2", 960, 2640))
        .keys(notes("F4 B4 D5", 960, 2640))
        .bass(notes("F2", 3600, 240))
        .keys(notes("E4 A4 C5", 3600, 240))
        .hear();
    assert_eq!(h.reported(), 8..24);
    let wraps: PcSet = h.notes().iter().filter(|n| n.wraps && !n.copy).map(|n| n.pitch_class()).collect();
    assert_eq!(wraps, pcs("F E A C"));
    // In the copy, it's struck in the last unit before the loop starts, over the G7 carried in.
    assert_eq!((h.struck(7, Stream::Harmony), h.struck(7, Stream::Bass)), (pcs("E A C"), pcs("F")));
    assert_eq!((h.struck(0, Stream::Bass), h.carried(0, Stream::Bass)), (PcSet::EMPTY, pcs("G")));
    assert_eq!(h.struck(8, Stream::Harmony), PcSet::EMPTY);
}

/// The music the invariance tests move about: a held and a walking bass, a chord held over, a push, a tune with held
/// and quick notes, a broken bar and a part of unknown role.
pub(super) fn with_bass() -> Music {
    Music::new()
        .bass([notes("C2", 0, BAR), line("D2 E2 F2 F#2", BAR, BEAT), notes("G2", 2 * BAR, BAR)].concat())
        .keys(
            [
                notes("C4 E4 G4 B4", 0, BAR + 100),
                notes("F3 A3 C4 E4", BAR, BAR - BEAT / 2),
                notes("G3 B3 D4 F4", 2 * BAR - BEAT / 2, BAR),
            ]
            .concat(),
        )
        .part(Role::Unknown, line("C3 E3 G3 C4 E4 C4 G3 E3", 3 * BAR, BEAT / 2))
        .tune([notes("E5", 0, BEAT), line("D5 C5 B4 C5", BEAT, BEAT / 2), notes("A5", BAR, 2 * BEAT)].concat())
}

/// And music with no bass part: the bass found by the gaps, an Alberti bar, a legato left hand.
pub(super) fn keys_only() -> Music {
    Music::new()
        .keys([notes("D3 C4 F4", 0, BAR), notes("G2 B3 F4", BAR, BAR + 10), notes("C3 B3 E4", 2 * BAR, BAR)].concat())
        .keys(line(&"C3 G3 E3 G3 ".repeat(4), 3 * BAR, BEAT / 4))
        .keys([notes("B2 E3 F3 A3", 4 * BAR, BEAT + 5), notes("C3 E3 G3", 4 * BAR + BEAT, BEAT)].concat())
}

/// What a hearing says about one unit, as text, from the parts its accessors give: a moved hearing is compared with
/// pitch classes moved back by `shift`, and a rescaled one with its ticks divided by `scale`.
#[allow(clippy::too_many_arguments)]
fn describe(
    level: Level,
    sound: u64,
    streams: [(PcSet, PcSet, PcSet); 3],
    broken: PcSet,
    onset: Option<PitchClass>,
    gap: Option<i32>,
    lowest: Option<i32>,
    masses: [[u64; 12]; 6],
) -> String {
    let streams: Vec<String> = streams.iter().map(|(s, c, h)| format!("{s}/{c}/{h}")).collect();
    let masses: Vec<String> = masses.iter().map(|m| format!("{m:?}")).collect();
    format!(
        "{level:?} S {sound} | {} | broken {broken} | onset {onset:?} | gap {gap:?} | lowest {lowest:?} | {}",
        streams.join(" | "),
        masses.join(" ")
    )
}

fn unit_facts(h: &Hearing, shift: i32, scale: u64) -> Vec<String> {
    let back = |s: PcSet| s.transposed(-shift);
    let scaled = |t: u64| {
        assert_eq!(t % scale, 0, "{t} ticks don't scale by {scale}");
        t / scale
    };
    (0..h.units())
        .map(|u| {
            let streams = Stream::ALL.map(|s| (back(h.struck(u, s)), back(h.carried(u, s)), back(h.sounding(u, s))));
            let masses = Mass::ALL.map(|m| h.mass(u..u + 1, m).rotated(PitchClass::new(shift)).0.map(scaled));
            describe(
                h.level(u),
                scaled(h.sounding_ticks(u..u + 1)),
                streams,
                back(h.broken_struck(u)),
                h.bass_onset(u..u + 1).map(|pc| pc.up(-shift)),
                h.gap_bass(u).map(|k| i32::from(k) - shift),
                h.lowest(u..u + 1).map(|k| i32::from(k) - shift),
                masses,
            )
        })
        .collect()
}

/// The notes heard, without where they came from (which part, which note), moved back and scaled.
fn heard_notes(h: &Hearing, shift: i32, scale: i64) -> Vec<String> {
    let mut out: Vec<String> = h
        .notes()
        .iter()
        .map(|n| {
            let held = n.held_over.map(|t| t / scale);
            let key = i32::from(n.key) - shift;
            let (start, end, length) = (n.start / scale, n.end / scale, n.length / scale);
            format!(
                "{key} {:?} {start}-{end} ({length}) {held:?} {} {} {} {}",
                n.stream, n.broken, n.copy, n.struck, n.wraps
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn moving_the_notes_moves_everything_heard_exactly() {
    for music in [with_bass(), keys_only(), with_bass().looped()] {
        let h = music.hear();
        let (facts, heard) = (unit_facts(&h, 0, 1), heard_notes(&h, 0, 1));
        for shift in -6..=5 {
            let moved = music.map(|n| Note { key: (i32::from(n.key) + shift) as u8, ..n }).hear();
            assert_eq!(unit_facts(&moved, shift, 1), facts, "moved {shift}");
            assert_eq!(heard_notes(&moved, shift, 1), heard, "moved {shift}");
            for bar in 0..h.units() / 8 {
                let texture = |t: BassTexture| match t {
                    BassTexture::Held(pc) => BassTexture::Held(pc.up(-shift)),
                    other => other,
                };
                assert_eq!(texture(moved.bass(8 * bar..8 * bar + 8)), h.bass(8 * bar..8 * bar + 8));
            }
        }
    }
}

#[test]
fn the_same_music_at_another_resolution_is_heard_the_same() {
    for music in [with_bass(), keys_only(), with_bass().looped()] {
        let h = music.hear();
        for ppq in [96, 960] {
            let scale = |t: i64| t * ppq / 480;
            let mut other = music.map(|n| Note { start: scale(n.start), length: scale(n.length), ..n });
            other.grid.ticks_per_beat = ppq as u32;
            let o = other.hear();
            let units = h.units();
            assert_eq!(o.units(), units);
            if ppq == 960 {
                assert_eq!(unit_facts(&o, 0, 2), unit_facts(&h, 0, 1));
                assert_eq!(heard_notes(&o, 0, 2), heard_notes(&h, 0, 1));
            } else {
                assert_eq!(unit_facts(&o, 0, 1), unit_facts(&h, 0, 5));
                assert_eq!(heard_notes(&o, 0, 1), heard_notes(&h, 0, 5));
            }
            for from in 0..units {
                for to in from + 1..=units.min(from + 16) {
                    assert_eq!(o.heard(from..to), h.heard(from..to));
                    assert_eq!(o.presence(from..to), h.presence(from..to));
                    assert_eq!(o.bass(from..to), h.bass(from..to));
                }
            }
        }
    }
}

#[test]
fn order_drums_doubles_quiet_velocities_and_ghost_notes_change_nothing() {
    for music in [with_bass(), keys_only(), with_bass().looped()] {
        let h = music.hear();
        let (facts, heard) = (unit_facts(&h, 0, 1), heard_notes(&h, 0, 1));
        // The notes and the parts in another order.
        let mut shuffled = music.clone();
        shuffled.parts.reverse();
        for (_, notes) in &mut shuffled.parts {
            notes.reverse();
            let third = notes.len() / 3;
            notes.rotate_left(third);
        }
        // A drum part.
        let drums = music.clone().part(Role::Drums, line("C2 D2 C2 D2 F#2 F#2", 0, BEAT / 2));
        // Every note played half as hard.
        let quiet = music.map(|n| Note { velocity: n.velocity / 2, ..n });
        // Ghost notes: short and very quiet.
        let mut ghosts = music.clone();
        for (_, notes) in &mut ghosts.parts {
            for i in 0..3 {
                notes.push(Note { start: i * 700 + 33, length: 20, key: 61 + i as u8, velocity: 300 });
            }
        }
        for (what, other) in [("shuffled", &shuffled), ("drums", &drums), ("quiet", &quiet), ("ghosts", &ghosts)] {
            let o = other.hear();
            assert_eq!(unit_facts(&o, 0, 1), facts, "{what}");
            assert_eq!(heard_notes(&o, 0, 1), heard, "{what}");
        }
        // The chords doubled an octave up, in the same parts: more notes, heard the same.
        let mut doubled = music.clone();
        for (role, notes) in &mut doubled.parts {
            if matches!(role, Role::Harmony | Role::Unknown) {
                let up: Vec<Note> = notes.iter().map(|n| Note { key: n.key + 12, ..*n }).collect();
                notes.extend(up);
            }
        }
        assert_eq!(unit_facts(&doubled.hear(), 0, 1), facts, "doubled");
    }
}

#[test]
fn a_hearing_kept_and_used_again_hears_the_same() {
    let mut hearing = Hearing::new();
    for music in [keys_only(), with_bass().looped(), with_bass(), keys_only()] {
        assert!(music.hear_into(&mut hearing));
        assert_eq!(unit_facts(&hearing, 0, 1), unit_facts(&music.hear(), 0, 1));
        assert_eq!(heard_notes(&hearing, 0, 1), heard_notes(&music.hear(), 0, 1));
    }
}

/// Each unit worked out the slow way, from the notes heard: every note against every unit. The fast way ([`Hearing`])
/// only looks at the units where a note starts or ends, and must agree.
fn slow_unit_facts(h: &Hearing) -> Vec<String> {
    #[derive(Clone, Copy)]
    struct Piece {
        key: u8,
        stream: Stream,
        broken: bool,
        over: bool,
        held_tune: bool,
        start: i64,
        end: i64,
        onset: bool,
    }
    let stream_index = |s: Stream| Stream::ALL.iter().position(|&x| x == s).unwrap();
    let mass_index = |m: Mass| Mass::ALL.iter().position(|&x| x == m).unwrap();
    let felt = h.felt_ticks();
    let pieces: Vec<Piece> = h
        .notes()
        .iter()
        .flat_map(|n| {
            let split = n.held_over.unwrap_or(n.end).clamp(n.start, n.end);
            let held_tune = n.stream == Stream::Melody && n.length >= felt;
            let piece = |over: bool, start: i64, end: i64| Piece {
                key: n.key,
                stream: n.stream,
                broken: n.broken,
                over,
                held_tune,
                start,
                end,
                onset: !over && n.struck,
            };
            [piece(false, n.start, split), piece(true, split, n.end)]
        })
        .filter(|p| p.start < p.end)
        .collect();
    // The bars a bass part plays in: where one of its notes is struck, or sounds for a quarter of a unit or more.
    let bounds = |u: usize| (h.start(u) - h.tick(0), h.start(u + 1) - h.tick(0));
    let plays = |u: usize| {
        let (lo, hi) = bounds(u);
        pieces.iter().any(|p| {
            let struck = p.onset && (lo..hi).contains(&p.start);
            p.stream == Stream::Bass && !p.over && (struck || (p.end.min(hi) - p.start.max(lo)) * 4 >= hi - lo)
        })
    };
    let per_bar = h.bar_units();
    let bass_bars: Vec<bool> = (0..h.units().div_ceil(per_bar))
        .map(|b| (b * per_bar..((b + 1) * per_bar).min(h.units())).any(plays))
        .collect();
    let mut last_gap: Option<u8> = None;
    (0..h.units())
        .map(|u| {
            let (lo, hi) = bounds(u);
            let len = hi - lo;
            let overlap = |p: &Piece| p.end.min(hi) - p.start.max(lo);
            let gap = if bass_bars[u / per_bar] {
                None
            } else {
                let sounding: Vec<&Piece> =
                    pieces.iter().filter(|p| p.stream == Stream::Harmony && !p.over && overlap(p) * 4 >= len).collect();
                let unbroken = |l: &u8| sounding.iter().any(|p| p.key == *l && !p.broken);
                let low = sounding.iter().map(|p| p.key).min().filter(unbroken);
                let next = low.and_then(|l| sounding.iter().map(|p| p.key).filter(|&k| k % 12 != l % 12).min());
                match (low, next) {
                    (Some(l), Some(n)) => (n - l >= BASS_GAP).then_some(l),
                    (Some(l), None) => (last_gap == Some(l)).then_some(l),
                    _ => None,
                }
            };
            last_gap = gap;
            let mut masses = [[0u64; 12]; 6];
            let mut streams = [(PcSet::EMPTY, PcSet::EMPTY, PcSet::EMPTY); 3];
            let mut broken = PcSet::EMPTY;
            let mut onset: Option<(i64, u8)> = None;
            let (mut lowest, mut lowest_any): (Option<u8>, Option<u8>) = (None, None);
            for p in pieces.iter().filter(|p| overlap(p) > 0) {
                if !p.over {
                    lowest_any = Some(lowest_any.map_or(p.key, |l| l.min(p.key)));
                }
                let ticks = overlap(p) as u64;
                let to_bass =
                    p.stream == Stream::Harmony && !p.broken && !p.over && gap.is_some_and(|g| g % 12 == p.key % 12);
                let stream = if to_bass { Stream::Bass } else { p.stream };
                let s = stream_index(stream);
                let pc = PitchClass::of_key(p.key);
                let mut raise = |m: Mass| {
                    let cell = &mut masses[mass_index(m)][pc.index()];
                    *cell = (*cell).max(ticks);
                };
                match (p.stream, p.over) {
                    (Stream::Harmony, false) => raise(if to_bass { Mass::Bass } else { Mass::Harmony }),
                    (Stream::Harmony, true) => raise(Mass::HeldOver),
                    (Stream::Bass, false) => raise(Mass::Bass),
                    (Stream::Melody, false) => {
                        raise(Mass::Melody);
                        if p.held_tune {
                            raise(Mass::HeldMelody);
                        }
                    }
                    _ => {}
                }
                if !p.over && overlap(p) * 4 >= len {
                    streams[s].2 = streams[s].2.with(pc);
                    if stream != Stream::Melody {
                        lowest = Some(lowest.map_or(p.key, |l| l.min(p.key)));
                    }
                }
                if p.onset && (lo..hi).contains(&p.start) {
                    streams[s].0 = streams[s].0.with(pc);
                    if p.broken && p.stream == Stream::Harmony {
                        broken = broken.with(pc);
                    }
                    if stream == Stream::Bass && onset.is_none_or(|first| (p.start, p.key) < first) {
                        onset = Some((p.start, p.key));
                    }
                }
                if (p.start < lo && p.end > lo) || (p.start == lo && !p.onset) {
                    streams[s].1 = streams[s].1.with(pc);
                }
            }
            let weight = h.level(u).weight();
            let bass = masses[mass_index(Mass::Bass)];
            masses[mass_index(Mass::WeightedBass)] = bass.map(|t| t * weight);
            let (harmony, bass) = (streams[stream_index(Stream::Harmony)].2, streams[stream_index(Stream::Bass)].2);
            let sound = if harmony.union(bass).is_empty() { 0 } else { len as u64 };
            let onset = onset.map(|(_, k)| PitchClass::of_key(k));
            let lowest = lowest.or(lowest_any).map(i32::from);
            describe(h.level(u), sound, streams, broken, onset, gap.map(i32::from), lowest, masses)
        })
        .collect()
}

/// A small random number maker (xorshift), so the random music is the same on every run.
struct Dice(u64);

impl Dice {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len() as u64) as usize]
    }
}

fn random_music(dice: &mut Dice) -> Music {
    let (beats, tpb) =
        dice.pick(&[(4, 480), (3, 480), (2, 96), (6, 240), (12, 240), (9, 120), (5, 96), (4, 7), (4, 1)]);
    let (beat, bar) = (i64::from(tpb), i64::from(beats) * i64::from(tpb));
    let mut music = Music::new().grid(beats, tpb);
    if dice.below(3) == 0 {
        music = music.looped();
    }
    let roles = [Role::Bass, Role::Melody, Role::Harmony, Role::Harmony, Role::Unknown, Role::Drums];
    for _ in 0..1 + dice.below(4) {
        let role = dice.pick(&roles);
        let mut notes = Vec::new();
        for _ in 0..dice.below(24) {
            let mut start = dice.below(4 * bar as u64 + 1) as i64;
            if dice.below(2) == 0 {
                let step = (beat / 2).max(1);
                start = start / step * step;
            }
            let lengths = [1, 5, beat / 4, beat / 2, beat - 1, beat, beat + 1, 2 * beat, bar + 10, 3 * bar];
            let length = dice.pick(&lengths).max(1);
            let key = 24 + dice.below(80) as u8;
            let velocity = dice.pick(&[100, 3000, 32768, 60000]);
            notes.push(Note { start, length, key, velocity });
        }
        if dice.below(3) == 0 {
            // An arpeggio: a run of short notes, one at a time.
            let step = (beat / 2).max(1);
            let from = dice.below(2 * bar as u64) as i64 / step * step;
            for i in 0..8 {
                notes.push(Note {
                    start: from + i * step,
                    length: step,
                    key: 48 + dice.below(24) as u8,
                    velocity: 32768,
                });
            }
        }
        music = music.part(role, notes);
    }
    music
}

#[test]
fn the_fast_way_agrees_with_the_slow_way() {
    let mut dice = Dice(0x2545_f491_4f6c_dd1d);
    let mut hearing = Hearing::new();
    let mut heard = 0;
    for _ in 0..400 {
        let music = random_music(&mut dice);
        if !music.hear_into(&mut hearing) {
            continue;
        }
        heard += 1;
        assert_eq!(unit_facts(&hearing, 0, 1), slow_unit_facts(&hearing), "{music:?}");
    }
    assert!(heard > 300, "only {heard} of the random pieces had notes");
    // And the hand-made music.
    for music in [with_bass(), keys_only(), with_bass().looped(), keys_only().looped()] {
        let h = music.hear();
        assert_eq!(unit_facts(&h, 0, 1), slow_unit_facts(&h));
    }
}

#[test]
fn damaged_notes_are_heard_without_a_panic() {
    let wild = [
        Note { start: i64::MAX - 5, length: i64::MAX, key: 255, velocity: 0 },
        Note { start: i64::MIN, length: -7, key: 127, velocity: u16::MAX },
        Note { start: 0, length: 0, key: 0, velocity: 1 },
        Note { start: 10_000_000 * 480, length: 480, key: 60, velocity: VELOCITY },
        Note { start: 10_000_000 * 480 + 240, length: 480, key: 131, velocity: VELOCITY },
    ];
    let mut hearing = Hearing::new();
    let grids = [(4, 480), (0, 0), (u32::MAX, u32::MAX), (6, 1), (12, 2), (1, u32::MAX)];
    for (beats_per_bar, ticks_per_beat) in grids {
        for bar_line in [0, i64::MIN, i64::MAX, 77] {
            let grid = Grid { ticks_per_beat, beats_per_bar, bar_line };
            for span in Span::ALL {
                for role in [Role::Harmony, Role::Bass, Role::Melody, Role::Unknown] {
                    let parts = [Part { role, notes: &wild }, Part { role: Role::Harmony, notes: &wild[2..] }];
                    assert!(hearing.hear(&parts, grid, span));
                    let units = hearing.units();
                    assert!(units > 0 && units <= MOST_UNITS, "{units} units");
                    assert!(hearing.truncated());
                    let all = 0..units;
                    for u in all.clone().step_by(97).chain([units - 1, units, units + 5]) {
                        let _ = (hearing.start(u), hearing.level(u), hearing.gap_bass(u), hearing.broken_struck(u));
                        for s in Stream::ALL {
                            let _ = (hearing.struck(u, s), hearing.carried(u, s), hearing.sounding(u, s));
                        }
                    }
                    for m in Mass::ALL {
                        let _ = hearing.mass(all.clone(), m);
                    }
                    let _ = (hearing.presence(all.clone()), hearing.heard(all.clone()), hearing.bass(all.clone()));
                    let _ = (hearing.bass_onset(all.clone()), hearing.struck_in(all.clone(), Stream::Harmony));
                    assert!(
                        hearing
                            .notes()
                            .iter()
                            .all(|n| n.start < n.end && n.end <= hearing.start(units) - hearing.tick(0))
                    );
                }
            }
        }
    }
    // Nothing pitched: nothing heard, and no units.
    assert!(!hearing.hear(&[], Grid::four_four(480), Span::Song));
    assert_eq!((hearing.units(), hearing.reported()), (0, 0..0));
    let drums = notes("C2 D2", 0, 480);
    assert!(!hearing.hear(&[Part { role: Role::Drums, notes: &drums }], Grid::four_four(480), Span::Loop));
    assert_eq!(hearing.heard(0..10), PcSet::EMPTY);
    assert_eq!(hearing.bass(0..10), BassTexture::None);
}
