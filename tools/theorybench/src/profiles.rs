//! Published key profiles, run the same way as the C# app's finder (how long each note sounds, matched to each key's
//! profile), so the bench shows how much the profile alone is worth. Only for measuring; the app never uses them.

use mo_harmony::Key;

use crate::baseline;
use crate::item::Note;

/// How well each note fits a key, tonic first, major then minor, as each paper gives them.
pub const PROFILES: [(&str, [f64; 12], [f64; 12]); 4] = [
    (
        // Krumhansl and Kessler (1982): listeners rating how well each note fits after a key was set up.
        "Krumhansl-Kessler profile",
        [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88],
        [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17],
    ),
    (
        // Temperley (2007): how often each note appears in the Kostka-Payne harmony textbook's examples.
        "Temperley (Kostka-Payne)",
        [0.748, 0.060, 0.488, 0.082, 0.670, 0.460, 0.096, 0.715, 0.104, 0.366, 0.057, 0.400],
        [0.712, 0.084, 0.474, 0.618, 0.049, 0.460, 0.105, 0.747, 0.404, 0.067, 0.133, 0.330],
    ),
    (
        // Aarden (2003): how often each note appears in the Essen folk song melodies.
        "Aarden-Essen profile",
        [
            17.7661, 0.145624, 14.9265, 0.160186, 19.8049, 11.3587, 0.291248, 22.062, 0.145624, 8.15494, 0.232998,
            4.95122,
        ],
        [18.2648, 0.737619, 14.0499, 16.8599, 0.702494, 14.4362, 0.702494, 18.6161, 4.56621, 1.93186, 7.37619, 1.75623],
    ),
    (
        // Bellman (2005), from Budge's (1943) counts of chords in classical music.
        "Bellman-Budge profile",
        [16.80, 0.86, 12.95, 1.41, 13.49, 11.93, 1.25, 20.28, 1.80, 8.04, 0.62, 10.57],
        [18.16, 0.69, 12.99, 13.34, 1.07, 11.15, 1.38, 21.07, 7.49, 1.53, 0.92, 10.21],
    ),
];

/// The key whose profile number `which` best matches these notes, with the C# app's way of being sure.
pub fn key(which: usize, notes: &[Note], bar: i64) -> Option<(Key, f64)> {
    let (_, major, minor) = PROFILES.get(which)?;
    baseline::estimate(&baseline::chroma(notes, bar), major, minor)
}
