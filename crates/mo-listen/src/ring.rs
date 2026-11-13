//! The ring the audio thread writes into and the ear reads from: a fixed number of samples, newest wins. Nothing in it
//! waits or locks: each sample is an atomic number, and a count of the samples written tells the ear where the newest
//! one is.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// How many samples the ring holds (mono): 0.68 s at 48 kHz, room for several of the ear's FFTs, so an ear that runs
/// late still finds a whole window of the newest audio.
pub const RING: usize = 1 << 15;
const MASK: u64 = RING as u64 - 1;

pub(crate) struct Ring {
    /// The samples, as their f32 bits; sample `n` of the stream sits at `n % RING`.
    samples: Box<[AtomicU32]>,
    /// How many samples have been written since the start: the next one is sample `written`.
    written: AtomicU64,
}

impl Ring {
    pub(crate) fn new() -> Ring {
        Ring { samples: (0..RING).map(|_| AtomicU32::new(0)).collect(), written: AtomicU64::new(0) }
    }

    /// How many samples have been written so far.
    pub(crate) fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Puts samples `from..to` of the stream into `out` (cleared first), as far as the ring still holds them; a
    /// sample from before the stream began reads as silence. Returns the first sample put in. A sample the audio
    /// thread overwrote while this read (only when the ear fell a whole ring behind) is left out, so what's put in
    /// is always one unbroken run of the stream.
    pub(crate) fn read(&self, from: u64, to: u64, out: &mut Vec<f32>) -> u64 {
        out.clear();
        let from = from.max(to.saturating_sub(RING as u64));
        out.extend((from..to).map(|n| f32::from_bits(self.samples[(n & MASK) as usize].load(Ordering::Relaxed))));
        // Anything before written - RING may have been overwritten while this copied.
        let lost = (self.written().saturating_sub(RING as u64).saturating_sub(from) as usize).min(out.len());
        out.drain(..lost);
        from + lost as u64
    }

    /// The newest `n` samples up to sample `to` into `out`, oldest first, with silence before the stream began.
    pub(crate) fn newest(&self, to: u64, n: usize, out: &mut Vec<f32>) {
        let n = n.min(RING);
        let missing = (n as u64).saturating_sub(to) as usize;
        out.clear();
        out.resize(missing, 0.0);
        let from = to - (n - missing) as u64;
        out.extend((from..to).map(|s| f32::from_bits(self.samples[(s & MASK) as usize].load(Ordering::Relaxed))));
    }
}

/// The audio thread's half: copies each block into the ring, mixed to mono. It never waits, locks, takes memory or
/// panics, whatever it's given: a block of any length (one longer than the ring keeps its newest part), channels of
/// different lengths (each sample is the average of the channels that have it), no channels at all, or numbers that
/// aren't numbers (they go in as silence).
pub struct Tap {
    ring: Arc<Ring>,
    channels: usize,
    /// How many samples (frames) this tap has been given so far.
    at: u64,
}

impl Tap {
    pub(crate) fn new(ring: Arc<Ring>, channels: usize) -> Tap {
        Tap { ring, channels, at: 0 }
    }

    /// Copies one block: `channels[c][i]` is sample `i` of channel `c`.
    pub fn push(&mut self, channels: &[&[f32]]) {
        let used = &channels[..channels.len().min(self.channels)];
        let frames = used.iter().map(|c| c.len()).max().unwrap_or(0);
        if frames == 0 {
            return;
        }
        // Only the newest RING frames can still be in the ring once this block is in.
        for i in frames.saturating_sub(RING)..frames {
            let (mut sum, mut n) = (0f32, 0f32);
            for c in used {
                if let Some(v) = c.get(i) {
                    sum += *v;
                    n += 1.0;
                }
            }
            // n is at least 1: the longest channel has every frame.
            let v = sum / n;
            let v = if v.is_finite() { v } else { 0.0 };
            self.ring.samples[((self.at + i as u64) & MASK) as usize].store(v.to_bits(), Ordering::Relaxed);
        }
        self.at += frames as u64;
        self.ring.written.store(self.at, Ordering::Release);
    }

    /// How many samples (frames) this tap has been given since it was made: the stream position the ear's
    /// [`crate::Heard::samples_to`] counts in.
    pub fn frames(&self) -> u64 {
        self.at
    }
}
