//! Listening to a track's audio live: the shared analyser behind the listener plugin and the main plugin's "listen to
//! its own track".
//!
//! It comes in two halves, made together by [`tap`]:
//!
//! - the [`Tap`], for the host's audio thread. It only copies each block (mixed to mono) into a fixed ring: it never
//!   waits, takes memory or works anything out, so it costs the audio thread close to nothing;
//! - the [`Ear`], for a low-priority thread. About every 100 ms it sums up the newest audio as one [`Heard`]: the
//!   level, how the sound splits between low, mid and high, the 12 notes heard (chroma) and the hits.
//!
//! A [`Track`] adds the summaries up over a song: the notes heard in each bar, and a rough tempo from the hits for
//! when the host gives none. The key and chords from what a track heard come in a later step, from the harmony engine.
//!
//! Plain Rust with no window, host or files, so it's tested with made-up sounds.

mod ear;
mod hits;
mod ring;
mod track;

pub use ear::{BANDS_HZ, Ear, FFT, Heard, QUIET};
pub use hits::Hit;
pub use ring::{RING, Tap};
pub use track::{Bar, MOST_BARS, Track};

use std::sync::Arc;

/// The two halves of one listener: the [`Tap`] for the audio thread and the [`Ear`] for a low-priority thread.
/// `sample_rate` is the host's (in Hz); `channels` is how many of a block's channels are mixed (at least 1: a block's
/// further channels are left out).
pub fn tap(sample_rate: f64, channels: usize) -> (Tap, Ear) {
    let ring = Arc::new(ring::Ring::new());
    (Tap::new(Arc::clone(&ring), channels.max(1)), Ear::new(ring, sample_rate))
}

/// A sample rate that can be one: the host's, or 48 kHz when it gave none that makes sense.
fn rate(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 }
}
