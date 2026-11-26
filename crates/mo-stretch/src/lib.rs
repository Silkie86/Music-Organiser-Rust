//! Stretching a sound to another tempo, or moving its pitch, without the other changing.
//!
//! The core is Signalsmith Stretch (MIT, by Geraint Luff / Signalsmith Audio), turned into Rust line by line so it's
//! ours to change: [`Stretch`]. The 1.1 plan builds on it (hits kept untouched, pre-echo taken away, the low end
//! locked); until Solomon has picked by ear, nothing in the app uses it.
//!
//! Sound in and out is one `f32` slice per channel. Nothing here reads or writes a file, and nothing waits: the work
//! is all sums, so a caller runs it off any thread that mustn't wait.

pub mod drums;
mod fft;
pub mod score;
mod stft;
mod stretch;

pub use stretch::Stretch;

/// `channels` (one slice per channel, all as long) stretched to `length` samples, its pitch moved by `semitones`,
/// with the original's default windows for `sample_rate`. Starts and ends as the sound does. `None` when the sound
/// is too short for the windows: under about 0.12 s, more when it's squashed (0.18 s at 1.5x as fast).
pub fn to_length(channels: &[&[f32]], sample_rate: f32, length: usize, semitones: f32) -> Option<Vec<Vec<f32>>> {
    let input_samples = channels.iter().map(|c| c.len()).min().unwrap_or(0);
    let mut out = vec![vec![0.0f32; length]; channels.len()];
    if channels.is_empty() || input_samples == 0 || length == 0 {
        return None;
    }
    let mut stretch = Stretch::new(channels.len(), sample_rate);
    if semitones != 0.0 {
        stretch.set_transpose_semitones(semitones, 0.0);
    }
    let mut outputs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
    stretch.exact(channels, input_samples, &mut outputs, length).then_some(out)
}
