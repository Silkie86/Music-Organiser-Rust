//! Cutting a sound into overlapping windows and turning each into bands, then adding the changed bands back into
//! sound (Signalsmith Linear's `DynamicSTFT`, from its `stft.h`): only the parts the stretcher uses. The windows are
//! Kaiser windows, the bands half-bin shifted ([`crate::fft`]), and the output is scaled so that a window taken and
//! put straight back gives the sound as it was ("round trip").
//!
//! The input and output are rings: the input keeps the last block of sound (and a little more), the output adds up
//! the windows that overlap the samples still to be read.

use crate::fft::{RealFft, size_above};
use rustfft::num_complex::Complex32;

/// Never divided by: an output sample no window has reached yet is read as 0, not as 0 / 0.
const ALMOST_ZERO: f32 = 1e-30;

/// The ring the input goes into: kept apart so it can be saved and put back (`Clone`).
#[derive(Clone, Default)]
pub(crate) struct Input {
    pos: usize,
    buffer: Vec<f32>,
}

/// The ring the output is added up in, with how much window has reached each sample.
#[derive(Clone, Default)]
struct Output {
    pos: usize,
    buffer: Vec<f32>,
    window_products: Vec<f32>,
}

pub(crate) struct Stft {
    fft: RealFft,
    channels: usize,
    input_length: usize,
    block: usize,
    fft_samples: usize,
    bands: usize,
    interval: usize,
    analysis_window: Vec<f32>,
    synthesis_window: Vec<f32>,
    analysis_offset: usize,
    synthesis_offset: usize,
    spectrum: Vec<Complex32>,
    time: Vec<f32>,
    pub(crate) input: Input,
    output: Output,
}

impl Stft {
    /// `channels` channels in and out, windows of `block` samples every `interval`, and `extra_history` more input
    /// kept than a block.
    pub(crate) fn new(channels: usize, block: usize, interval: usize, extra_history: usize) -> Self {
        let fft_samples = size_above(block.div_ceil(2)) * 2;
        let bands = fft_samples / 2;
        let input_length = block + extra_history;
        let mut stft = Self {
            fft: RealFft::new(fft_samples),
            channels,
            input_length,
            block,
            fft_samples,
            bands,
            interval,
            analysis_window: vec![0.0; block],
            synthesis_window: vec![0.0; block],
            analysis_offset: 0,
            synthesis_offset: 0,
            spectrum: vec![Complex32::new(0.0, 0.0); bands * channels],
            time: vec![0.0; fft_samples.max(block)],
            input: Input { pos: 0, buffer: vec![0.0; input_length * channels] },
            output: Output { pos: 0, buffer: vec![0.0; block * channels], window_products: vec![0.0; block] },
        };
        stft.set_kaiser_windows();
        stft.reset(1.0);
        stft
    }

    pub(crate) fn block(&self) -> usize {
        self.block
    }
    pub(crate) fn fft_samples(&self) -> usize {
        self.fft_samples
    }
    pub(crate) fn interval(&self) -> usize {
        self.interval
    }
    pub(crate) fn bands(&self) -> usize {
        self.bands
    }
    pub(crate) fn analysis_latency(&self) -> usize {
        self.block - self.analysis_offset
    }
    pub(crate) fn synthesis_latency(&self) -> usize {
        self.synthesis_offset
    }
    /// The frequency of band `b`, as a share of the sample rate.
    pub(crate) fn band_to_freq(&self, b: f32) -> f32 {
        (b + 0.5) / self.fft_samples as f32
    }
    pub(crate) fn freq_to_band(&self, f: f32) -> f32 {
        f * self.fft_samples as f32 - 0.5
    }

    /// A Kaiser window as wide as the interval asks for, made to add up to exactly 1 over the overlaps, used both to
    /// take windows and to put them back.
    fn set_kaiser_windows(&mut self) {
        let beta = kaiser_beta(self.block as f64 / self.interval as f64);
        let inv_b0 = 1.0 / bessel0(beta);
        let size = self.block;
        let inv_size = 1.0 / size as f64;
        let offset_i = if size & 1 == 1 { 1 } else { 0 };
        for (i, w) in self.synthesis_window.iter_mut().enumerate() {
            let r = (2 * i + offset_i) as f64 * inv_size - 1.0;
            let arg = (1.0 - r * r).sqrt();
            *w = (bessel0(beta * arg) * inv_b0) as f32;
        }
        // Make the windows' squares add up to 1 at every sample, wherever the windows overlap.
        for i in 0..self.interval {
            // As the original: each square in f32, added up in f64.
            let sum2: f64 = (i..size)
                .step_by(self.interval)
                .map(|j| (self.synthesis_window[j] * self.synthesis_window[j]) as f64)
                .sum();
            let factor = 1.0 / sum2.sqrt();
            for j in (i..size).step_by(self.interval) {
                self.synthesis_window[j] = (self.synthesis_window[j] as f64 * factor) as f32;
            }
        }
        self.analysis_window.clone_from(&self.synthesis_window);
        // A window's middle is its highest point (the first one found from the middle on).
        self.analysis_offset = size / 2;
        self.synthesis_offset = size / 2;
        for i in 0..size {
            if self.analysis_window[i] > self.analysis_window[self.analysis_offset] {
                self.analysis_offset = i;
            }
            if self.synthesis_window[i] > self.synthesis_window[self.synthesis_offset] {
                self.synthesis_offset = i;
            }
        }
    }

    /// Empties the input and output. `product_weight` below 1 makes the first output quieter while too few windows
    /// overlap it to be sure of it.
    pub(crate) fn reset(&mut self, product_weight: f32) {
        self.input.pos = self.block;
        self.output.pos = 0;
        self.input.buffer.fill(0.0);
        self.output.buffer.fill(0.0);
        self.spectrum.fill(Complex32::new(0.0, 0.0));
        self.output.window_products.fill(0.0);
        self.add_window_product();
        for i in (0..self.block.saturating_sub(self.interval)).rev() {
            self.output.window_products[i] += self.output.window_products[i + self.interval];
        }
        for v in &mut self.output.window_products {
            *v = *v * product_weight + ALMOST_ZERO;
        }
        self.move_output(self.interval);
    }

    pub(crate) fn write_input(&mut self, channel: usize, samples: &[f32]) {
        let length = self.input_length;
        let buffer = &mut self.input.buffer[channel * length..(channel + 1) * length];
        for (i, &s) in samples.iter().enumerate() {
            buffer[(self.input.pos + i) % length] = s;
        }
    }

    pub(crate) fn move_input(&mut self, samples: usize) {
        self.input.pos = (self.input.pos + samples) % self.input_length;
    }

    /// Lets the output fade to 0 by its windows when no more windows will come.
    pub(crate) fn finish_output(&mut self) {
        let mut most = 0.0f32;
        for i in 0..self.block {
            let wp = &mut self.output.window_products[(self.output.pos + i) % self.block];
            most = cmax(*wp, most);
            *wp += most - *wp;
        }
    }

    /// `out.len()` samples of `channel`'s output, starting `offset` samples on.
    pub(crate) fn read_output(&self, channel: usize, offset: usize, out: &mut [f32]) {
        let buffer = &self.output.buffer[channel * self.block..(channel + 1) * self.block];
        for (i, o) in out.iter_mut().enumerate() {
            let i2 = (self.output.pos + offset + i) % self.block;
            *o = buffer[i2] / self.output.window_products[i2];
        }
    }

    /// One output sample of `channel`, the next to be read.
    pub(crate) fn read_output_one(&self, channel: usize) -> f32 {
        let pos = self.output.pos;
        self.output.buffer[channel * self.block + pos] / self.output.window_products[pos]
    }

    /// Adds `samples` into `channel`'s output from where it's read next, as if they had come out of the windows.
    pub(crate) fn add_output(&mut self, channel: usize, samples: &[f32]) {
        let length = samples.len().min(self.block);
        let buffer = &mut self.output.buffer[channel * self.block..(channel + 1) * self.block];
        for (i, &s) in samples.iter().take(length).enumerate() {
            let i2 = (self.output.pos + i) % self.block;
            buffer[i2] += s * self.output.window_products[i2];
        }
    }

    /// Moves the output on, clearing what's been read.
    pub(crate) fn move_output(&mut self, samples: usize) {
        for i in 0..samples {
            let i2 = (self.output.pos + i) % self.block;
            for c in 0..self.channels {
                self.output.buffer[i2 + c * self.block] = 0.0;
            }
            self.output.window_products[i2] = ALMOST_ZERO;
        }
        self.output.pos = (self.output.pos + samples) % self.block;
    }

    pub(crate) fn spectrum(&self, channel: usize) -> &[Complex32] {
        &self.spectrum[channel * self.bands..(channel + 1) * self.bands]
    }
    pub(crate) fn spectrum_mut(&mut self, channel: usize) -> &mut [Complex32] {
        &mut self.spectrum[channel * self.bands..(channel + 1) * self.bands]
    }

    /// The bands of every channel's latest window, or of the one `samples_in_past` samples before it.
    pub(crate) fn analyse(&mut self, samples_in_past: usize) {
        for channel in 0..self.channels {
            let length = self.input_length;
            let offset_pos = (length * 2 + self.input.pos - self.block - samples_in_past) % length;
            let buffer = &self.input.buffer[channel * length..(channel + 1) * length];
            let time_size = self.time.len();
            for i in 0..self.block {
                let w = self.analysis_window[i];
                let s = buffer[(offset_pos + i) % length];
                if i < self.analysis_offset {
                    // The window's first half goes round to the end, turned over: half-shifted bands repeat upside
                    // down.
                    self.time[i + time_size - self.analysis_offset] = -s * w;
                } else {
                    self.time[i - self.analysis_offset] = s * w;
                }
            }
            for i in (self.block - self.analysis_offset)..(self.fft_samples - self.analysis_offset) {
                self.time[i] = 0.0;
            }
            let bands = self.bands;
            self.fft
                .forward(&self.time[..self.fft_samples], &mut self.spectrum[channel * bands..(channel + 1) * bands]);
        }
    }

    /// Turns every channel's bands back into sound and adds it to the output.
    pub(crate) fn synthesise(&mut self) {
        self.add_window_product();
        for channel in 0..self.channels {
            let bands = self.bands;
            self.fft
                .inverse(&self.spectrum[channel * bands..(channel + 1) * bands], &mut self.time[..self.fft_samples]);
            let buffer = &mut self.output.buffer[channel * self.block..(channel + 1) * self.block];
            for i in 0..self.block {
                let w = self.synthesis_window[i];
                let bi = (self.output.pos + i) % self.block;
                if i < self.synthesis_offset {
                    buffer[bi] -= self.time[i + self.fft_samples - self.synthesis_offset] * w;
                } else {
                    buffer[bi] += self.time[i - self.synthesis_offset] * w;
                }
            }
        }
    }

    /// Adds one window's worth of gain to the samples it reaches, so the output can be divided by it.
    fn add_window_product(&mut self) {
        let shift = self.synthesis_offset as isize - self.analysis_offset as isize;
        let w_min = shift.max(0) as usize;
        let w_max = (self.block as isize + shift).min(self.block as isize).max(0) as usize;
        let scaling = self.fft_samples as f32;
        for i in w_min..w_max {
            let wa = self.analysis_window[(i as isize - shift) as usize];
            let ws = self.synthesis_window[i];
            self.output.window_products[(self.output.pos + i) % self.block] += wa * ws * scaling;
        }
    }
}

/// `std::max` as C++ has it: the first unless the second is bigger (so a NaN second is passed over).
pub(crate) fn cmax(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

/// `std::min` as C++ has it: the first unless the second is smaller.
pub(crate) fn cmin(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}

/// The zeroth Bessel function, to 4 places: what a Kaiser window is made of.
fn bessel0(x: f64) -> f64 {
    let mut result = 0.0;
    let mut term = 1.0;
    let mut m = 0.0;
    while term > 1e-4 {
        result += term;
        m += 1.0;
        term *= (x * x) / (4.0 * m * m);
    }
    result
}

/// A Kaiser window's shape for a window `bandwidth` intervals wide, with Signalsmith's tuning found by search.
fn kaiser_beta(bandwidth: f64) -> f64 {
    let bandwidth = bandwidth + 8.0 / ((bandwidth + 3.0) * (bandwidth + 3.0)) + 0.25 * (3.0 - bandwidth).max(0.0);
    let bandwidth = bandwidth.max(2.0);
    let alpha = (bandwidth * bandwidth * 0.25 - 1.0).sqrt();
    alpha * std::f64::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window taken and put straight back, block after block, gives the sound back as it was, a latency later.
    #[test]
    fn round_trip_gives_the_sound_back() {
        let (block, interval) = (480, 120);
        let mut stft = Stft::new(1, block, interval, interval + 1);
        let sound: Vec<f32> =
            (0..6000).map(|i| (i as f32 * 0.05).sin() * 0.5 + ((i * 7 % 13) as f32 - 6.0) / 20.0).collect();
        let mut out = Vec::new();
        for chunk in sound.chunks(interval) {
            stft.write_input(0, chunk);
            stft.move_input(chunk.len());
            stft.analyse(0);
            stft.synthesise();
            let mut part = vec![0.0; chunk.len()];
            stft.read_output(0, 0, &mut part);
            out.extend(part);
            stft.move_output(chunk.len());
        }
        let latency = stft.analysis_latency() + stft.synthesis_latency() - interval;
        let mut worst = 0.0f32;
        for i in 2 * block..sound.len() - block {
            worst = worst.max((out[i] - sound[i - latency]).abs());
        }
        assert!(worst < 1e-4, "worst difference {worst}");
    }
}
