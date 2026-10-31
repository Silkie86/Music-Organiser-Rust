//! How far a plugin moves the notes it plays, read from its saved settings where their layout is known, so the part
//! guess hears the real pitch (spec 3.4a). Only Serum 1 so far; for any other plugin the answer is "not known", and
//! the guess says the octave wasn't checked.
//!
//! **Serum 1** (the C# app's `Serum1State`, worked out from Solomon's saves): its VST2 chunk and VST3 state are a zlib
//! stream that unpacks to 172,736 bytes (20,704 or more for a preset file). The knobs are 0-to-1 floats, little-endian,
//! from byte 13,408 in Serum's parameter order. Each oscillator keeps 13 knobs together from its level: level, pan,
//! octave, semi, fine, ... coarse pitch at the tenth. Oscillator A's level is knob 1, B's is 14; their on switches
//! are knobs 212 and 213, their key tracking 164 and 165. Octave is -4 to +4 in 9 steps, semi -12 to +12 in 25, coarse
//! pitch -64 to +64 semitones. The sub oscillator has its level at 33, its on switch at 215 and its octave at 171.

use std::io::Read;

use flate2::read::ZlibDecoder;

/// How many semitones the plugin moves its notes, from its saved settings (as the reader gives them: an .fxp, a
/// .vstpreset, or the raw state); `None` when it isn't a plugin whose settings can be read for that.
pub fn plugin_shift(plugin: &str, settings: &[u8]) -> Option<f64> {
    if is_serum_1(plugin) {
        return serum_1_shift(settings);
    }
    None
}

/// Serum 1's names ("Serum", "Serum_x64", "Serum (x64)"), not Serum 2's.
fn is_serum_1(plugin: &str) -> bool {
    let p = plugin.trim().to_lowercase();
    p.starts_with("serum") && !p.starts_with("serum 2") && !p.starts_with("serum2") && !p.starts_with("serumfx")
}

/// A whole Serum 1 state, unpacked.
const SERUM_SIZE: usize = 172_736;
/// The least a preset file holds.
const SERUM_PRESET_SIZE: usize = 20_704;
const KNOBS_AT: usize = 13_408;
/// An .fxp's chunk starts here (after "CcnK", its kind, the plugin id, the program name and the chunk's size).
const FXP_CHUNK_AT: usize = 60;
/// A .vstpreset's component state starts here (after "VST3", its version, the class id and where the list is).
const VSTPRESET_STATE_AT: usize = 48;

/// Oscillators A and B: (level knob, on switch knob, key tracking switch knob), as the C# `Serum1To2` maps them.
const SERUM_OSCILLATORS: [(usize, usize, usize); 2] = [(1, 212, 164), (14, 213, 165)];
/// The sub oscillator: level, on switch and octave knobs (it has no semi or coarse pitch).
const SERUM_SUB: (usize, usize, usize) = (33, 215, 171);

/// The pitch move of Serum 1's loudest oscillator that's on and follows the keys (the lower one when two are as loud):
/// octave, semi and coarse pitch, in semitones. When neither A nor B plays, the sub oscillator's octave.
fn serum_1_shift(settings: &[u8]) -> Option<f64> {
    let raw = serum_1_unpacked(settings)?;
    let knob = |i: usize| -> Option<f64> {
        let at = KNOBS_AT.checked_add(i.checked_mul(4)?)?;
        let b = raw.get(at..at + 4)?.first_chunk::<4>()?;
        let v = f32::from_le_bytes(*b);
        v.is_finite().then_some(f64::from(v.clamp(0.0, 1.0)))
    };
    // Serum 1's switches read "on" for any value above 0 (as the C# `Serum1To2` reads them).
    let on = |i: usize| knob(i).map(|v| v > 0.0);
    // Serum rounds a stepped knob to the nearest step, halves away from zero.
    let steps = |x: f64, range: f64| (x * 2.0 * range).round() - range;
    let mut best: Option<(f64, f64)> = None;
    for (level_at, on_at, track_at) in SERUM_OSCILLATORS {
        // Off, or playing the same pitch whatever key is pressed: its knobs don't move the notes.
        if !on(on_at)? || !on(track_at)? {
            continue;
        }
        let level = knob(level_at)?;
        let shift = 12.0 * steps(knob(level_at + 2)?, 4.0)
            + steps(knob(level_at + 3)?, 12.0)
            + (2.0 * knob(level_at + 9)? - 1.0) * 64.0;
        let louder = match best {
            None => true,
            Some((l, s)) => level > l || (level == l && shift < s),
        };
        if louder && level > 0.0 {
            best = Some((level, shift));
        }
    }
    if let Some((_, shift)) = best {
        return Some(shift);
    }
    // Only the sub (or noise) playing: the sub's octave, when it's on and heard.
    let (level_at, on_at, octave_at) = SERUM_SUB;
    if on(on_at)? && knob(level_at)? > 0.0 {
        return Some(12.0 * steps(knob(octave_at)?, 4.0));
    }
    // Nothing that follows the keys plays, so nothing here moves the pitch.
    Some(0.0)
}

/// Serum 1's state, unpacked, from an .fxp (chunk after its header), a .vstpreset (state after its header), or FL's
/// raw copy (a few bytes of its own first): the zlib stream is found where it starts.
fn serum_1_unpacked(settings: &[u8]) -> Option<Vec<u8>> {
    let from = if settings.starts_with(b"CcnK") {
        FXP_CHUNK_AT
    } else if settings.starts_with(b"VST3") {
        VSTPRESET_STATE_AT
    } else {
        0
    };
    let rest = settings.get(from..)?;
    // A zlib header: 0x78, and the two bytes as a number divisible by 31 (Serum packs with the lowest setting, 78 01).
    // The bytes in front can look like one too, so each place that does is tried until one unpacks.
    rest.windows(2)
        .take(64)
        .enumerate()
        .filter(|(_, w)| matches!(w, [0x78, b] if (0x78u16 << 8 | u16::from(*b)).is_multiple_of(31)))
        .find_map(|(start, _)| {
            let mut out = Vec::with_capacity(SERUM_SIZE);
            ZlibDecoder::new(rest.get(start..)?).take(SERUM_SIZE as u64 + 1).read_to_end(&mut out).ok()?;
            (out.len() >= SERUM_PRESET_SIZE && out.len() <= SERUM_SIZE).then_some(out)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    /// A made-up Serum 1 state with these knob values (by index), packed as Serum packs it.
    fn serum_state(knobs: &[(usize, f32)]) -> Vec<u8> {
        let mut raw = vec![0u8; SERUM_SIZE];
        for &(i, v) in knobs {
            raw[KNOBS_AT + 4 * i..KNOBS_AT + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        z.write_all(&raw).unwrap();
        z.finish().unwrap()
    }

    /// Oscillator A on, following the keys and full, at this octave knob position; B off.
    fn osc_a(octave: f32, semi: f32, coarse: f32) -> Vec<(usize, f32)> {
        vec![(212, 1.0), (213, 0.0), (164, 1.0), (1, 0.75), (3, octave), (4, semi), (10, coarse)]
    }

    #[test]
    fn reads_serum_1s_oscillator_pitch() {
        let middle = osc_a(0.5, 0.5, 0.5);
        assert_eq!(plugin_shift("Serum_x64", &serum_state(&middle)), Some(0.0));
        // Octave -2 (knob 2/8), semi +7 (19/24), coarse in the middle.
        let low = osc_a(0.25, 19.0 / 24.0, 0.5);
        assert_eq!(plugin_shift("Serum", &serum_state(&low)), Some(-17.0));
        // In an .fxp, after its 60-byte header.
        let mut fxp = b"CcnK".to_vec();
        fxp.resize(60, 0);
        fxp.extend(serum_state(&low));
        assert_eq!(plugin_shift("Serum", &fxp), Some(-17.0));
        // FL's own few bytes in front.
        let mut fl = vec![1, 0, 0, 0, 9];
        fl.extend(serum_state(&low));
        assert_eq!(plugin_shift("Serum", &fl), Some(-17.0));
    }

    #[test]
    fn the_louder_oscillator_that_is_on_counts() {
        // A an octave up but quiet, B two octaves down and louder.
        let both = [
            (212, 1.0),
            (213, 1.0),
            (1, 0.3),
            (3, 0.625),
            (4, 0.5),
            (10, 0.5),
            (14, 0.9),
            (16, 0.25),
            (17, 0.5),
            (23, 0.5),
            (164, 1.0),
            (165, 1.0),
        ];
        assert_eq!(plugin_shift("Serum", &serum_state(&both)), Some(-24.0));
        // B switched off: A's.
        let mut a_only = both.to_vec();
        a_only[1] = (213, 0.0);
        assert_eq!(plugin_shift("Serum", &serum_state(&a_only)), Some(12.0));
        // B on but not following the keys: it doesn't move the notes, so A's.
        let mut b_fixed = both.to_vec();
        b_fixed[11] = (165, 0.0);
        assert_eq!(plugin_shift("Serum", &serum_state(&b_fixed)), Some(12.0));
        // A switch reads on for any value above 0.
        let mut b_half_on = both.to_vec();
        b_half_on[1] = (213, 0.25);
        assert_eq!(plugin_shift("Serum", &serum_state(&b_half_on)), Some(-24.0));
    }

    #[test]
    fn only_the_sub_playing_gives_its_octave() {
        // A and B off, the sub on and heard, two octaves down (knob 2/8).
        let sub = [(212, 0.0), (213, 0.0), (215, 1.0), (33, 0.75), (171, 0.25)];
        assert_eq!(plugin_shift("Serum", &serum_state(&sub)), Some(-24.0));
        // Nothing on at all: nothing moves the pitch.
        assert_eq!(plugin_shift("Serum", &serum_state(&[])), Some(0.0));
    }

    #[test]
    fn a_false_zlib_header_in_front_is_passed_over() {
        // 78 01 in FL's own bytes before the real stream.
        let low = osc_a(0.25, 19.0 / 24.0, 0.5);
        let mut fl = vec![0x78, 0x01, 0, 0, 9];
        fl.extend(serum_state(&low));
        assert_eq!(plugin_shift("Serum", &fl), Some(-17.0));
    }

    #[test]
    fn other_plugins_and_damaged_states_arent_known() {
        let state = serum_state(&osc_a(0.5, 0.5, 0.5));
        assert_eq!(plugin_shift("Serum 2", &state), None);
        assert_eq!(plugin_shift("Sylenth1", &state), None);
        assert_eq!(plugin_shift("Serum", b"not a state"), None);
        assert_eq!(plugin_shift("Serum", &state[..state.len() / 2]), None, "cut short");
    }
}
