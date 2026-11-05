//! Audition on the Presets page: the short part a preset plays in Bitwig while you listen to it, picked by what the
//! preset is for (its part, as you set it or the page guessed). Each is two or four bars in A minor at the song's
//! tempo, made here: nothing is learned from anyone's MIDI (rule 5). Bitwig needs a file, so the part is written as
//! "MO Play <pid>-<n>.mid" in Temp, as Play in Bitwig's is, and goes again once Bitwig has read it (rule 28).

use mo_formats::{MidiClip, NoteAttribute, NoteEvent};
use mo_midi::{BAR, PPQ};
use mo_presets::role::Role;

/// A sixteenth note, in ticks at [`PPQ`].
const SIXTEENTH: i64 = PPQ / 4;
/// An eighth note.
const EIGHTH: i64 = PPQ / 2;

/// A note at `start` for `length` ticks, at MIDI 1.0's velocity `vel7` given MIDI 2.0's 16 bits (rule 22).
fn note(start: i64, length: i64, key: u8, vel7: u8) -> NoteEvent {
    NoteEvent {
        start,
        length,
        key,
        velocity: mo_formats::scale_up(u32::from(vel7), 7, 16) as u16,
        muted: false,
        step: false,
        attribute: NoteAttribute::NONE,
    }
}

/// Chords held for `each` ticks one after another, every one of them `length` long, from the bar's start.
fn chords(shapes: &[&[u8]], each: i64, length: i64, vel7: u8) -> Vec<NoteEvent> {
    let mut out = Vec::new();
    for (n, shape) in shapes.iter().enumerate() {
        let start = n as i64 * each;
        out.extend(shape.iter().map(|&k| note(start, length, k, vel7)));
    }
    out
}

/// A minor, F, C, G: the chords most parts below follow (A3 and up, so they sit in the middle).
const AM: &[u8] = &[57, 60, 64];
const F: &[u8] = &[53, 57, 60];
const C: &[u8] = &[55, 60, 64];
const G: &[u8] = &[55, 59, 62];

/// The part for a preset of this kind and how many bars it loops over: a bass line for a bass, a tune for a lead, held
/// chords for a pad, short hits for stabs, an arpeggio for an arp, a beat (General MIDI drum keys) for drums, one long
/// note for FX, and the chords for chords or when the part isn't known.
pub fn part(role: Option<Role>) -> (Vec<NoteEvent>, i32) {
    match role {
        Some(Role::Bass) => {
            // Eighths on A1, F1, C2 and G1, each with an octave jump at the end of its half bar.
            let roots = [33u8, 29, 36, 31];
            let mut out = Vec::new();
            for (n, root) in roots.iter().enumerate() {
                let half = n as i64 * BAR / 2;
                for step in 0..4 {
                    let key = if step == 3 { root + 12 } else { *root };
                    out.push(note(half + step * EIGHTH, EIGHTH - SIXTEENTH / 2, key, if step == 0 { 110 } else { 92 }));
                }
            }
            (out, 2)
        }
        Some(Role::Lead) => {
            // A short tune in A minor, around A4.
            let tune: [(i64, i64, u8); 9] = [
                (0, 2, 69),
                (2, 2, 72),
                (4, 2, 76),
                (6, 2, 74),
                (8, 4, 72),
                (12, 2, 71),
                (14, 2, 67),
                (16, 8, 69),
                (24, 8, 64),
            ];
            (tune.iter().map(|&(at, len, key)| note(at * EIGHTH, len * EIGHTH - SIXTEENTH / 2, key, 100)).collect(), 4)
        }
        Some(Role::Pad) => (chords(&[AM, F, C, G], BAR, BAR, 85), 4),
        Some(Role::Stabs) => {
            // Each chord hit short on the off-beats of its half bar.
            let mut out = Vec::new();
            for (n, shape) in [AM, F, C, G].iter().enumerate() {
                for hit in [1, 3] {
                    let start = n as i64 * BAR / 2 + hit * EIGHTH;
                    out.extend(shape.iter().map(|&k| note(start, SIXTEENTH, k, 105)));
                }
            }
            (out, 2)
        }
        Some(Role::Arp) => {
            // Sixteenths up and down each chord, an octave added on top.
            let mut out = Vec::new();
            for (n, shape) in [AM, F, C, G].iter().enumerate() {
                let keys = [shape[0], shape[1], shape[2], shape[0] + 12, shape[2], shape[1], shape[0], shape[1]];
                for (s, key) in keys.iter().enumerate() {
                    let start = n as i64 * BAR / 2 + s as i64 * SIXTEENTH;
                    out.push(note(start, SIXTEENTH, *key, if s == 0 { 105 } else { 90 }));
                }
            }
            (out, 2)
        }
        Some(Role::Drums) => {
            // Kick (36) on one and three, snare (38) on two and four, closed hats (42) on the eighths.
            let mut out = Vec::new();
            for bar in 0..2 {
                let at = bar * BAR;
                for beat in [0, 2] {
                    out.push(note(at + beat * PPQ, SIXTEENTH, 36, 115));
                }
                for beat in [1, 3] {
                    out.push(note(at + beat * PPQ, SIXTEENTH, 38, 110));
                }
                for e in 0..8 {
                    out.push(note(at + e * EIGHTH, SIXTEENTH, 42, if e % 2 == 0 { 90 } else { 70 }));
                }
            }
            (out, 2)
        }
        Some(Role::Fx) => (vec![note(0, 2 * BAR, 60, 100)], 4),
        Some(Role::Chords) | None => (chords(&[AM, F, C, G], BAR / 2, BAR / 2 - SIXTEENTH, 95), 2),
    }
}

/// The part as a MIDI clip for Bitwig, named after the preset.
pub fn clip(name: &str, role: Option<Role>) -> MidiClip {
    let (notes, bars) = part(role);
    crate::midi::part_clip(name, &notes, bars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_part_fits_its_bars_and_plays() {
        for role in Role::ALL.iter().copied().map(Some).chain([None]) {
            let (notes, bars) = part(role);
            assert!(!notes.is_empty(), "{role:?}");
            let end = i64::from(bars) * BAR;
            assert!(notes.iter().all(|n| n.start >= 0 && n.length > 0 && n.start + n.length <= end), "{role:?}");
            assert!(notes.iter().all(|n| n.velocity > 0 && n.key <= 127), "{role:?}");
        }
    }

    #[test]
    fn parts_sit_where_their_sounds_do() {
        let lowest = |r| part(Some(r)).0.iter().map(|n| n.key).min().unwrap_or(0);
        assert!(lowest(Role::Bass) < 40, "a bass plays low");
        assert!(lowest(Role::Lead) >= 60, "a lead plays a tune over the middle");
        // Drums use General MIDI's kick, snare and hat keys.
        let drums: std::collections::HashSet<u8> = part(Some(Role::Drums)).0.iter().map(|n| n.key).collect();
        assert_eq!(drums, [36, 38, 42].into_iter().collect());
        // Velocities keep MIDI 2.0's resolution and give the MIDI 1.0 ones back.
        assert_eq!(part(Some(Role::Drums)).0[0].velocity7(), 115);
    }

    #[test]
    fn the_clip_loops_over_its_bars() {
        let clip = clip("Warm Pad", Some(Role::Pad));
        assert_eq!(clip.name, "Warm Pad");
        assert_eq!(clip.end, Some(4 * BAR));
    }
}
