//! The C# app's key finder as it was (`MusicTheory.EstimateMidiKey` and `ProjectKey.Estimate`, at the commit the twin
//! test measures), copied here unchanged as a fixed point: every new idea is measured against it on the same music.
//! It is never used by the app.

use mo_harmony::{Key, PitchClass};

use crate::item::Note;

/// How long each note sounds in real songs, tonic first: the C# app's average of POP909 and gospel, jazz, neo soul and
/// soul packs (`MidiMajorProfile`, `MidiMinorProfile`).
const MAJOR: [f64; 12] = [17.91, 1.30, 12.96, 2.03, 14.03, 8.23, 1.84, 17.60, 1.96, 12.91, 2.72, 6.50];
const MINOR: [f64; 12] = [20.41, 1.69, 7.63, 13.60, 1.69, 12.15, 1.50, 17.53, 6.78, 2.45, 12.13, 2.46];

/// Below this the Songs page showed no key (`ProjectKey.MinConfidence`).
pub const SONG_MIN_CONFIDENCE: f64 = 0.12;

/// The key the C# app heard in these notes and its confidence (0 to 1), as `ProjectKey.Estimate` worked it out before
/// its confidence cut: `None` with fewer than 8 notes or fewer than 4 pitch classes.
pub fn key(notes: &[Note], bar: i64) -> Option<(Key, f64)> {
    let chroma = chroma(notes, bar);
    if notes.len() < 8 || chroma.iter().filter(|&&c| c > 0.0).count() < 4 {
        return None;
    }
    estimate(&chroma, &MAJOR, &MINOR)
}

/// How long each pitch class sounds, in bars, each note counted between a sixteenth of a bar and two bars: a long
/// pad counts more than a passing note, but not without limit.
pub fn chroma(notes: &[Note], bar: i64) -> [f64; 12] {
    let mut chroma = [0.0f64; 12];
    let bar = bar.max(1) as f64;
    for n in notes {
        if let Some(c) = chroma.get_mut(PitchClass::of_key(n.key).index()) {
            *c += (n.length as f64 / bar).clamp(1.0 / 16.0, 2.0);
        }
    }
    chroma
}

/// `MusicTheory.Estimate` with no first chord: the key whose profile the chroma matches best, and the C# app's
/// confidence in it.
pub fn estimate(chroma: &[f64; 12], major: &[f64; 12], minor: &[f64; 12]) -> Option<(Key, f64)> {
    if chroma.iter().sum::<f64>() <= 1e-9 {
        return None;
    }
    let mut scores: Vec<(Key, f64)> = Vec::with_capacity(24);
    for tonic in PitchClass::ALL {
        scores.push((Key::major(tonic), correlate(chroma, major, tonic)));
        scores.push((Key::minor(tonic), correlate(chroma, minor, tonic)));
    }
    // C#'s List.Sort isn't stable; ties between two keys' scores don't happen with real notes.
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (best, r) = *scores.first()?;
    // How strongly it matches, and how clearly it beats the runner-up that isn't its relative major or minor (those
    // share every note).
    let relative = best.relative();
    let runner = scores.iter().skip(1).find(|(k, _)| *k != relative)?.1;
    let confidence = r.clamp(0.0, 1.0) * ((r - runner) * 5.0).clamp(0.0, 1.0);
    Some((best, (confidence * 1000.0).round() / 1000.0))
}

/// Pearson correlation of the chroma, turned so `tonic` comes first, with a profile.
fn correlate(chroma: &[f64; 12], profile: &[f64; 12], tonic: PitchClass) -> f64 {
    let mx = chroma.iter().sum::<f64>() / 12.0;
    let my = profile.iter().sum::<f64>() / 12.0;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (i, &p) in profile.iter().enumerate() {
        let x = chroma.get(tonic.up(i as i32).index()).copied().unwrap_or_default() - mx;
        let y = p - my;
        sxy += x * y;
        sxx += x * x;
        syy += y * y;
    }
    if sxx <= 0.0 { 0.0 } else { sxy / (sxx * syy).sqrt() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(bar: i64, keys: &[u8]) -> Vec<Note> {
        keys.iter().map(|&key| Note { start: bar * 384, length: 384, key, velocity: 100 }).collect()
    }

    #[test]
    fn the_c_sharp_tests_still_pass() {
        // ProjectKeyTests: Am, Dm, E, Am is A minor; one chord is too few notes.
        let a_minor: Vec<Note> =
            [held(0, &[57, 60, 64]), held(1, &[62, 65, 69]), held(2, &[64, 68, 71]), held(3, &[57, 60, 64])].concat();
        let (k, c) = key(&a_minor, 384).unwrap();
        assert_eq!(k.name(), "A minor");
        assert!(c >= SONG_MIN_CONFIDENCE);
        assert_eq!(key(&held(0, &[57, 60, 64]), 384), None);
    }
}
