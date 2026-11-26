//! Signalsmith Stretch, turned into Rust (version 1.3.2 of `signalsmith-stretch.h`, MIT, by Geraint Luff /
//! Signalsmith Audio: <https://github.com/Signalsmith-Audio/signalsmith-stretch>).
//!
//! How it works, in short: the sound is cut into overlapping windows, each turned into bands (a phase vocoder). For
//! each new window of output it takes the input's bands from where the new tempo puts it, keeps their strength, and
//! works out each band's phase so it carries on smoothly from the last output window. A plain phase vocoder only
//! looks back in time for that, which smears drums and makes tones "phasey"; this one also lines each band up with
//! its neighbours in the strongest channel, and locks the other channels to it, so a stereo picture stays put.
//! Pitch moves by mapping each band to a new frequency, peak by peak; formants (a voice's or instrument's tone) can
//! be kept where they were or moved on their own.
//!
//! Kept as the original, so it can be checked against it: the same window sizes, bands and sums, in f32. Left out:
//! spreading a window's work over several output samples (`splitComputation`, for a plug-in's tight time budget).
//! Changed: the random numbers used past a 2x stretch come from our own small generator (the original's is the C++
//! standard library's), so they differ from the original's but are the same every run.

use crate::stft::{Stft, cmax, cmin};
use rustfft::num_complex::Complex32;
use std::f32::consts::PI;

/// Quieter than this is silence.
const NOISE_FLOOR: f32 = 1e-15;
/// The stretch (2x as long) past which phases are given a little chance, so a long stretch doesn't buzz.
const MAX_CLEAN_STRETCH: f32 = 2.0;
const ZERO: Complex32 = Complex32 { re: 0.0, im: 0.0 };

#[derive(Clone, Copy, Default)]
struct Band {
    input: Complex32,
    prev_input: Complex32,
    output: Complex32,
    input_energy: f32,
}

#[derive(Clone, Copy, Default)]
struct Prediction {
    energy: f32,
    input: Complex32,
}

impl Prediction {
    /// The output band: `phase`'s direction at this band's strength.
    fn make_output(&self, phase: Complex32) -> Complex32 {
        let mut phase = phase;
        let mut phase_norm = phase.norm_sqr();
        if phase_norm <= NOISE_FLOOR {
            // Too weak to go by: take the input's phase instead.
            phase = self.input;
            phase_norm = self.input.norm_sqr() + NOISE_FLOOR;
        }
        phase * (self.energy / phase_norm).sqrt()
    }
}

#[derive(Clone, Copy)]
struct Peak {
    input: f32,
    output: f32,
}

#[derive(Clone, Copy, Default)]
struct MapPoint {
    input_bin: f32,
    freq_grad: f32,
}

/// What the next window of output does, worked out when it starts.
#[derive(Clone, Copy)]
struct BlockProcess {
    samples_since_last: usize,
    new_spectrum: bool,
    reanalyse_prev: bool,
    mapped_frequencies: bool,
    process_formants: bool,
    time_factor: f32,
}

impl Default for BlockProcess {
    fn default() -> Self {
        Self {
            samples_since_last: usize::MAX,
            new_spectrum: false,
            reanalyse_prev: false,
            mapped_frequencies: false,
            process_formants: false,
            time_factor: 1.0,
        }
    }
}

/// How each input frequency moves: by a factor, or by a map of the caller's.
enum FreqMap {
    Factor,
    Custom(Box<dyn Fn(f32) -> f32 + Send>),
}

/// A small random number generator (xorshift64*): the same numbers every run.
struct Random(u64);

impl Random {
    /// A number from `low` up to `high`.
    fn between(&mut self, low: f32, high: f32) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        let unit = (x >> 40) as f32 / (1u64 << 24) as f32;
        low + (high - low) * unit
    }
}

/// The stretcher for one sound of `channels` channels. Feed it with [`process`](Self::process) (as much input as
/// the output it's asked for should take: twice as much input as output plays it twice as fast), or do a whole
/// sound at once with [`exact`](Self::exact).
pub struct Stretch {
    stft: Stft,
    channels: usize,
    bands: usize,
    block_process: BlockProcess,
    silence_counter: usize,
    silence_first: bool,
    freq_multiplier: f32,
    freq_tonality_limit: f32,
    freq_map: FreqMap,
    formant_compensation: bool,
    formant_multiplier: f32,
    inv_formant_multiplier: f32,
    formant_base_freq: f32,
    prev_input_offset: i64,
    did_seek: bool,
    seek_time_factor: f32,
    channel_bands: Vec<Band>,
    peaks: Vec<Peak>,
    energy: Vec<f32>,
    smoothed_energy: Vec<f32>,
    output_map: Vec<MapPoint>,
    channel_predictions: Vec<Prediction>,
    formant_metric: Vec<f32>,
    freq_estimate_weighted: f32,
    freq_estimate_weight: f32,
    freq_estimate: f32,
    random: Random,
    /// Scratch room, kept so a window's work doesn't allocate.
    tmp: Vec<f32>,
}

impl Stretch {
    /// The original's default: 120 ms windows every 30 ms.
    pub fn new(channels: usize, sample_rate: f32) -> Self {
        Self::with_sizes(channels, (sample_rate * 0.12) as usize, (sample_rate * 0.03) as usize)
    }

    /// The original's cheaper setting: 100 ms windows every 40 ms (less work, a little less clean).
    pub fn cheaper(channels: usize, sample_rate: f32) -> Self {
        Self::with_sizes(channels, (sample_rate * 0.1) as usize, (sample_rate * 0.04) as usize)
    }

    /// Windows of `block` samples, a new one every `interval` samples. `channels`, `block` and `interval` are at
    /// least 1 (smaller is taken as 1); `interval` is at most `block`.
    pub fn with_sizes(channels: usize, block: usize, interval: usize) -> Self {
        let channels = channels.max(1);
        let block = block.max(4);
        let interval = interval.clamp(1, block);
        let mut stft = Stft::new(channels, block, interval, interval + 1);
        stft.reset(0.1);
        let bands = stft.bands();
        Self {
            channels,
            bands,
            block_process: BlockProcess::default(),
            silence_counter: 0,
            silence_first: true,
            freq_multiplier: 1.0,
            freq_tonality_limit: 0.5,
            freq_map: FreqMap::Factor,
            formant_compensation: false,
            formant_multiplier: 1.0,
            inv_formant_multiplier: 1.0,
            formant_base_freq: 0.0,
            prev_input_offset: -1,
            did_seek: false,
            seek_time_factor: 1.0,
            channel_bands: vec![Band::default(); bands * channels],
            peaks: Vec::with_capacity(bands / 2),
            energy: vec![0.0; bands],
            smoothed_energy: vec![0.0; bands],
            output_map: vec![MapPoint::default(); bands],
            channel_predictions: vec![Prediction::default(); channels * bands],
            formant_metric: vec![0.0; bands + 2],
            freq_estimate_weighted: 0.0,
            freq_estimate_weight: 0.0,
            freq_estimate: 0.0,
            random: Random(0x9E37_79B9_7F4A_7C15),
            tmp: vec![0.0; block + interval],
            stft,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn block_samples(&self) -> usize {
        self.stft.block()
    }
    pub fn interval_samples(&self) -> usize {
        self.stft.interval()
    }
    /// How far into the input the middle of the window being worked on is: input arrives this early.
    pub fn input_latency(&self) -> usize {
        self.stft.analysis_latency()
    }
    /// How far behind the output comes.
    pub fn output_latency(&self) -> usize {
        self.stft.synthesis_latency()
    }

    /// Forgets all sound so far.
    pub fn reset(&mut self) {
        self.stft.reset(0.1);
        self.prev_input_offset = -1;
        self.channel_bands.fill(Band::default());
        self.silence_counter = 0;
        self.did_seek = false;
        self.block_process = BlockProcess::default();
        self.freq_estimate_weighted = 0.0;
        self.freq_estimate_weight = 0.0;
    }

    /// Moves every frequency by `multiplier` (2 is an octave up). Above `tonality_limit` (a share of the sample
    /// rate; 0 for none) frequencies are moved by the same amount instead of the same factor, which keeps noise and
    /// air from sounding too thin or too dull.
    pub fn set_transpose_factor(&mut self, multiplier: f32, tonality_limit: f32) {
        self.freq_multiplier = multiplier;
        self.freq_tonality_limit = if tonality_limit > 0.0 { tonality_limit / multiplier.sqrt() } else { 1.0 };
        self.freq_map = FreqMap::Factor;
    }

    /// [`set_transpose_factor`](Self::set_transpose_factor) in semitones.
    pub fn set_transpose_semitones(&mut self, semitones: f32, tonality_limit: f32) {
        self.set_transpose_factor(2f32.powf(semitones / 12.0), tonality_limit);
    }

    /// Moves frequencies by a map of the caller's (a share of the sample rate in and out), which must only go up.
    pub fn set_freq_map(&mut self, input_to_output: impl Fn(f32) -> f32 + Send + 'static) {
        self.freq_map = FreqMap::Custom(Box::new(input_to_output));
    }

    /// Moves formants by `multiplier`. With `compensate_pitch` the formants stay where they were when the pitch moves
    /// (before `multiplier`).
    pub fn set_formant_factor(&mut self, multiplier: f32, compensate_pitch: bool) {
        self.formant_multiplier = multiplier;
        self.inv_formant_multiplier = 1.0 / multiplier;
        self.formant_compensation = compensate_pitch;
    }

    /// [`set_formant_factor`](Self::set_formant_factor) in semitones.
    pub fn set_formant_semitones(&mut self, semitones: f32, compensate_pitch: bool) {
        self.set_formant_factor(2f32.powf(semitones / 12.0), compensate_pitch);
    }

    /// A rough guess at the sound's lowest note (a share of the sample rate), used to find its formants; 0 has it
    /// guessed from the sound.
    pub fn set_formant_base(&mut self, base_freq: f32) {
        self.formant_base_freq = base_freq;
    }

    /// How much input [`seek`](Self::seek) would like.
    pub fn seek_length(&self) -> usize {
        self.stft.block() + self.stft.interval()
    }

    /// Gives the stretcher the input before where it should start (its last [`seek_length`](Self::seek_length)
    /// samples are used), so it can start there without a run-up. `playback_rate` is how fast the input will be
    /// played (2 is twice as fast).
    pub fn seek(&mut self, inputs: &[&[f32]], input_samples: usize, playback_rate: f64) {
        let size = self.stft.block() + self.stft.interval();
        self.tmp.clear();
        self.tmp.resize(size, 0.0);
        let start = input_samples.saturating_sub(size);
        let pad_start = size + start - input_samples;
        let mut total_energy = 0.0f32;
        for (c, input) in inputs.iter().enumerate().take(self.channels) {
            for i in start..input_samples {
                let s = input[i];
                total_energy += s * s;
                self.tmp[i - start + pad_start] = s;
            }
            self.stft.write_input(c, &self.tmp);
        }
        self.stft.move_input(size);
        if total_energy >= NOISE_FLOOR {
            self.silence_counter = 0;
            self.silence_first = true;
        }
        self.did_seek = true;
        let interval = self.stft.interval() as f64;
        self.seek_time_factor =
            if playback_rate * interval > 1.0 { (1.0 / playback_rate) as f32 } else { interval as f32 };
    }

    /// How much input [`output_seek`](Self::output_seek) takes at `playback_rate`.
    pub fn output_seek_length(&self, playback_rate: f32) -> usize {
        (self.input_latency() as f32 + playback_rate * self.output_latency() as f32) as usize
    }

    /// Starts afresh at the beginning of `inputs` and works out the run-up, so the very next output is the sound's
    /// start. `input_length` should be [`output_seek_length`](Self::output_seek_length) (the stretch is worked out
    /// from it).
    pub fn output_seek(&mut self, inputs: &[&[f32]], input_length: usize) {
        self.reset();
        let surplus = input_length.saturating_sub(self.input_latency());
        let latency = self.output_latency();
        let playback_rate = surplus as f32 / latency as f32;
        let seek_samples = input_length - surplus;
        self.seek(inputs, seek_samples, playback_rate as f64);

        // The extra input makes the run-up: worked out, then turned round and taken away, so what's left starts as
        // the sound does.
        let mut pre_roll = vec![vec![0.0f32; latency]; self.channels];
        {
            let offset: Vec<&[f32]> = inputs.iter().take(self.channels).map(|c| &c[seek_samples..]).collect();
            let mut outs: Vec<&mut [f32]> = pre_roll.iter_mut().map(Vec::as_mut_slice).collect();
            self.process(&offset, surplus, &mut outs, latency);
        }
        for (c, channel) in pre_roll.iter_mut().enumerate() {
            for v in channel.iter_mut() {
                *v = -*v;
            }
            channel.reverse();
            self.stft.add_output(c, channel);
        }
    }

    /// Takes `input_samples` of input and gives `output_samples` of output: the ratio is the stretch. `inputs` and
    /// `outputs` hold one slice per channel, at least that long.
    pub fn process(
        &mut self,
        inputs: &[&[f32]],
        input_samples: usize,
        outputs: &mut [&mut [f32]],
        output_samples: usize,
    ) {
        let channels = self.channels.min(inputs.len()).min(outputs.len());
        let mut prev_copied = 0usize;

        let mut total_energy = 0.0f32;
        for input in inputs.iter().take(channels) {
            for &s in &input[..input_samples] {
                total_energy += s * s;
            }
        }

        if total_energy < NOISE_FLOOR {
            if self.silence_counter >= 2 * self.stft.block() {
                if self.silence_first {
                    self.silence_first = false;
                    self.block_process = BlockProcess::default();
                    for b in &mut self.channel_bands {
                        b.input = ZERO;
                        b.prev_input = ZERO;
                        b.output = ZERO;
                        b.input_energy = 0.0;
                    }
                }
                for c in 0..channels {
                    for o in 0..output_samples {
                        outputs[c][o] = if input_samples > 0 { inputs[c][o % input_samples] } else { 0.0 };
                    }
                }
                self.copy_input(inputs, &mut prev_copied, input_samples);
                return;
            }
            self.silence_counter += input_samples;
        } else {
            self.silence_counter = 0;
            self.silence_first = true;
        }

        let interval = self.stft.interval();
        for output_index in 0..output_samples {
            if self.block_process.samples_since_last >= interval {
                self.start_block(inputs, &mut prev_copied, input_samples, output_index, output_samples);
            }
            self.block_process.samples_since_last += 1;
            for (c, output) in outputs.iter_mut().enumerate().take(channels) {
                output[output_index] = self.stft.read_output_one(c);
            }
            self.stft.move_output(1);
        }

        self.copy_input(inputs, &mut prev_copied, input_samples);
        self.prev_input_offset -= input_samples as i64;
    }

    /// Puts input up to `to_index` into the window ring (at most a block and an interval of it: older would fall
    /// out anyway).
    fn copy_input(&mut self, inputs: &[&[f32]], prev_copied: &mut usize, to_index: usize) {
        let length = (self.stft.block() + self.stft.interval()).min(to_index.saturating_sub(*prev_copied));
        let offset = to_index - length;
        for (c, input) in inputs.iter().enumerate().take(self.channels) {
            self.stft.write_input(c, &input[offset..to_index]);
        }
        self.stft.move_input(length);
        *prev_copied = to_index;
    }

    /// A new window of output: where in the input it comes from, then its bands.
    fn start_block(
        &mut self,
        inputs: &[&[f32]],
        prev_copied: &mut usize,
        input_samples: usize,
        output_index: usize,
        output_samples: usize,
    ) {
        let interval = self.stft.interval();
        self.block_process.samples_since_last = 0;
        // Worked out in the original's order, so it rounds the same.
        let input_offset = (output_index as f32 * input_samples as f32 / output_samples as f32).round() as i64;
        let input_interval = input_offset - self.prev_input_offset;
        self.prev_input_offset = input_offset;
        self.copy_input(inputs, prev_copied, input_offset.max(0) as usize);

        let bp = &mut self.block_process;
        bp.new_spectrum = self.did_seek || input_interval > 0;
        bp.mapped_frequencies = matches!(self.freq_map, FreqMap::Custom(_)) || self.freq_multiplier != 1.0;
        if bp.new_spectrum {
            // The last window must be an interval back (give or take a sample), or it's taken again.
            bp.reanalyse_prev = self.did_seek || (input_interval - interval as i64).abs() > 1;
        }
        bp.process_formants = self.formant_multiplier != 1.0 || (self.formant_compensation && bp.mapped_frequencies);
        bp.time_factor =
            if self.did_seek { self.seek_time_factor } else { interval as f32 / input_interval.max(1) as f32 };
        self.did_seek = false;

        if self.block_process.new_spectrum {
            if self.block_process.reanalyse_prev {
                self.stft.analyse(interval);
                for c in 0..self.channels {
                    let spectrum = self.stft.spectrum(c);
                    for (band, s) in self.channel_bands[c * self.bands..(c + 1) * self.bands].iter_mut().zip(spectrum) {
                        band.prev_input = *s;
                    }
                }
            }
            self.stft.analyse(0);
            for c in 0..self.channels {
                let spectrum = self.stft.spectrum(c);
                for (band, s) in self.channel_bands[c * self.bands..(c + 1) * self.bands].iter_mut().zip(spectrum) {
                    band.input = *s;
                }
            }
        }

        self.process_spectrum();

        for c in 0..self.channels {
            let bands = self.bands;
            let spectrum = self.stft.spectrum_mut(c);
            for (s, band) in spectrum.iter_mut().zip(&self.channel_bands[c * bands..(c + 1) * bands]) {
                *s = band.output;
            }
        }
        self.stft.synthesise();
    }

    /// The rest of the output, with no more input: past an interval of it, the input is taken as silence.
    pub fn flush(&mut self, outputs: &mut [&mut [f32]], output_samples: usize, playback_rate: f32) {
        let interval = self.stft.interval();
        let output_block = output_samples.saturating_sub(interval);
        if output_block > 0 {
            let zero_samples = (output_block as f32 * playback_rate) as usize;
            let zeros = vec![0.0f32; zero_samples];
            let inputs: Vec<&[f32]> = (0..self.channels).map(|_| zeros.as_slice()).collect();
            self.process(&inputs, zero_samples, outputs, output_block);
        }

        let tail = output_samples - output_block; // an interval at most
        self.tmp.clear();
        self.tmp.resize(tail, 0.0);
        self.stft.finish_output();
        for (c, output) in outputs.iter_mut().enumerate().take(self.channels) {
            self.stft.read_output(c, 0, &mut self.tmp);
            output[output_block..output_block + tail].copy_from_slice(&self.tmp);
            // The window that would come next, turned round and taken away: the end fades out as it should.
            self.stft.read_output(c, tail, &mut self.tmp);
            for (i, &v) in self.tmp.iter().enumerate() {
                output[output_block + tail - 1 - i] -= v;
            }
        }
        self.stft.reset(0.1);
        for band in &mut self.channel_bands {
            band.prev_input = ZERO;
            band.output = ZERO;
        }
    }

    /// A whole sound at once: `inputs` (each `input_samples` long) stretched to fill `outputs` (each
    /// `output_samples` long), starting and ending as the sound does. `false` (and silence) when the sound is too
    /// short for the windows.
    pub fn exact(
        &mut self,
        inputs: &[&[f32]],
        input_samples: usize,
        outputs: &mut [&mut [f32]],
        output_samples: usize,
    ) -> bool {
        let playback_rate = input_samples as f32 / output_samples as f32;
        let seek_length = self.output_seek_length(playback_rate);
        let output_index = output_samples as f32 - seek_length as f32 / playback_rate;
        if input_samples < seek_length || output_samples == 0 || output_index < 0.0 {
            for output in outputs.iter_mut() {
                output[..output_samples].fill(0.0);
            }
            return false;
        }
        let output_index = output_index as usize;
        self.output_seek(inputs, seek_length);

        let offset_inputs: Vec<&[f32]> = inputs.iter().map(|c| &c[seek_length..]).collect();
        self.process(&offset_inputs, input_samples - seek_length, outputs, output_index);

        let mut offset_outputs: Vec<&mut [f32]> = outputs.iter_mut().map(|c| &mut c[output_index..]).collect();
        self.flush(&mut offset_outputs, output_samples - output_index, playback_rate);
        true
    }

    // The band work for one window.

    fn band_index(&self, channel: usize, index: i64) -> Option<usize> {
        (index >= 0 && (index as usize) < self.bands).then(|| index as usize + channel * self.bands)
    }

    fn fractional_c(&self, channel: usize, low: i64, frac: f32, get: fn(&Band) -> Complex32) -> Complex32 {
        let low_v = self.band_index(channel, low).map_or(ZERO, |i| get(&self.channel_bands[i]));
        let high_v = self.band_index(channel, low + 1).map_or(ZERO, |i| get(&self.channel_bands[i]));
        low_v + (high_v - low_v) * frac
    }

    fn fractional_at(&self, channel: usize, index: f32, get: fn(&Band) -> Complex32) -> Complex32 {
        let low = index.floor();
        self.fractional_c(channel, low as i64, index - low, get)
    }

    fn fractional_energy(&self, channel: usize, low: i64, frac: f32) -> f32 {
        let low_v = self.band_index(channel, low).map_or(0.0, |i| self.channel_bands[i].input_energy);
        let high_v = self.band_index(channel, low + 1).map_or(0.0, |i| self.channel_bands[i].input_energy);
        low_v + (high_v - low_v) * frac
    }

    fn process_spectrum(&mut self) {
        let bands = self.bands;
        let channels = self.channels;
        let interval = self.stft.interval();
        let bp = self.block_process;

        let smoothing_bins = self.stft.fft_samples() as f32 / interval as f32;
        let long_step = smoothing_bins.round() as usize;
        let time_factor = cmax(bp.time_factor, 1.0 / MAX_CLEAN_STRETCH);
        let random_time_factor = time_factor > MAX_CLEAN_STRETCH;
        let random_low = if random_time_factor { MAX_CLEAN_STRETCH * 2.0 } else { 0.0 } - time_factor;

        if bp.new_spectrum {
            // Turn last window's output and input on by an interval's worth of each band's own frequency.
            let rot_start = polar(self.stft.band_to_freq(0.0) * interval as f32 * (2.0 * PI));
            let freq_step = self.stft.band_to_freq(1.0) - self.stft.band_to_freq(0.0);
            let rot_step = polar(freq_step * interval as f32 * (2.0 * PI));
            for c in 0..channels {
                let mut rot = rot_start;
                for bin in &mut self.channel_bands[c * bands..(c + 1) * bands] {
                    bin.output = mul(bin.output, rot);
                    bin.prev_input = mul(bin.prev_input, rot);
                    rot = mul(rot, rot_step);
                }
            }
        }

        if bp.mapped_frequencies {
            self.smooth_energy(smoothing_bins);
            self.find_peaks();
            self.update_output_map();
        } else {
            for bin in &mut self.channel_bands {
                bin.input_energy = bin.input.norm_sqr();
            }
            for (b, point) in self.output_map.iter_mut().enumerate() {
                *point = MapPoint { input_bin: b as f32, freq_grad: 1.0 };
            }
        }

        if bp.process_formants {
            self.update_formants();
        }

        // A first guess at each band from the phase vocoder: last output turned on by how much the input turned.
        for c in 0..channels {
            for b in 0..bands {
                let point = self.output_map[b];
                let low = point.input_bin.floor();
                let frac = point.input_bin - low;
                let low = low as i64;
                let prev_energy = self.channel_predictions[c * bands + b].energy;
                let energy = self.fractional_energy(c, low, frac) * point.freq_grad.max(0.0);
                let input = self.fractional_c(c, low, frac, |b| b.input);
                let prev_input = self.fractional_c(c, low, frac, |b| b.prev_input);
                let prediction = &mut self.channel_predictions[c * bands + b];
                prediction.energy = energy;
                prediction.input = input;
                let freq_twist = mul_conj(input, prev_input);
                let output_bin = &mut self.channel_bands[c * bands + b];
                let phase = mul(output_bin.output, freq_twist);
                output_bin.output = phase / (cmax(prev_energy, energy) + NOISE_FLOOR);
            }
        }

        // Then each band again, lined up with its neighbours in the loudest channel, and the other channels locked to
        // that one.
        for b in 0..bands {
            let mut max_channel = 0;
            let mut max_energy = self.channel_predictions[b].energy;
            for c in 1..channels {
                let e = self.channel_predictions[c * bands + b].energy;
                if e > max_energy {
                    max_channel = c;
                    max_energy = e;
                }
            }
            let mc = max_channel;
            let prediction = self.channel_predictions[mc * bands + b];
            let point = self.output_map[b];
            let mut phase = ZERO;

            // From the band below (and the one a long step below).
            if b > 0 {
                let btf = if random_time_factor { self.random.between(random_low, time_factor) } else { time_factor };
                let down_input = self.fractional_at(mc, point.input_bin - btf, |b| b.input);
                let short_twist = mul_conj(prediction.input, down_input);
                phase += mul(self.channel_bands[mc * bands + b - 1].output, short_twist);
                if b >= long_step {
                    let long_down = self.fractional_at(mc, point.input_bin - long_step as f32 * btf, |b| b.input);
                    let long_twist = mul_conj(prediction.input, long_down);
                    phase += mul(self.channel_bands[mc * bands + b - long_step].output, long_twist);
                }
            }
            // From the band above (and the one a long step above).
            if b + 1 < bands {
                let up_prediction = self.channel_predictions[mc * bands + b + 1];
                let up_point = self.output_map[b + 1];
                let btf = if random_time_factor { self.random.between(random_low, time_factor) } else { time_factor };
                let down_input = self.fractional_at(mc, up_point.input_bin - btf, |b| b.input);
                let short_twist = mul_conj(up_prediction.input, down_input);
                phase += mul_conj(self.channel_bands[mc * bands + b + 1].output, short_twist);
                if b + long_step < bands {
                    let long_prediction = self.channel_predictions[mc * bands + b + long_step];
                    let long_point = self.output_map[b + long_step];
                    let long_down = self.fractional_at(mc, long_point.input_bin - long_step as f32 * btf, |b| b.input);
                    let long_twist = mul_conj(long_prediction.input, long_down);
                    phase += mul_conj(self.channel_bands[mc * bands + b + long_step].output, long_twist);
                }
            }

            let output = prediction.make_output(phase);
            self.channel_bands[mc * bands + b].output = output;
            for c in 0..channels {
                if c != mc {
                    let channel_prediction = self.channel_predictions[c * bands + b];
                    let channel_twist = mul_conj(channel_prediction.input, prediction.input);
                    let channel_phase = mul(output, channel_twist);
                    self.channel_bands[c * bands + b].output = channel_prediction.make_output(channel_phase);
                }
            }
        }

        if bp.new_spectrum {
            for bin in &mut self.channel_bands {
                bin.prev_input = bin.input;
            }
        }
    }

    /// The bands' energy over all channels, and a smoothed copy: a band louder than the smooth line is part of a peak.
    fn smooth_energy(&mut self, smoothing_bins: f32) {
        let slew = 1.0 / (1.0 + smoothing_bins * 0.5);
        self.energy.fill(0.0);
        for c in 0..self.channels {
            for (b, bin) in self.channel_bands[c * self.bands..(c + 1) * self.bands].iter_mut().enumerate() {
                let e = bin.input.norm_sqr();
                bin.input_energy = e;
                self.energy[b] += e;
            }
        }
        self.smoothed_energy.copy_from_slice(&self.energy);
        let mut e = 0.0f32;
        for _ in 0..2 {
            for s in self.smoothed_energy.iter_mut().rev() {
                e += (*s - e) * slew;
                *s = e;
            }
            for s in self.smoothed_energy.iter_mut() {
                e += (*s - e) * slew;
                *s = e;
            }
        }
    }

    fn map_freq(&self, freq: f32) -> f32 {
        match &self.freq_map {
            FreqMap::Custom(map) => map(freq),
            FreqMap::Factor if freq > self.freq_tonality_limit => {
                freq + (self.freq_multiplier - 1.0) * self.freq_tonality_limit
            }
            FreqMap::Factor => freq * self.freq_multiplier,
        }
    }

    /// The peaks in the spectrum, each with where its frequency moves to.
    fn find_peaks(&mut self) {
        self.peaks.clear();
        let bands = self.bands;
        let mut start = 0;
        while start < bands {
            if self.energy[start] > self.smoothed_energy[start] {
                let mut end = start;
                let (mut band_sum, mut energy_sum) = (0.0f32, 0.0f32);
                while end < bands && self.energy[end] > self.smoothed_energy[end] {
                    band_sum += end as f32 * self.energy[end];
                    energy_sum += self.energy[end];
                    end += 1;
                }
                let avg_band = band_sum / energy_sum;
                let avg_freq = self.stft.band_to_freq(avg_band);
                let output = self.stft.freq_to_band(self.map_freq(avg_freq));
                self.peaks.push(Peak { input: avg_band, output });
                start = end;
            }
            start += 1;
        }
    }

    /// For each output band, the input band it comes from: peaks move as the map says, the bands between them are
    /// spread smoothly.
    fn update_output_map(&mut self) {
        let bands = self.bands as i64;
        let (Some(&first), Some(&last)) = (self.peaks.first(), self.peaks.last()) else {
            for (b, point) in self.output_map.iter_mut().enumerate() {
                *point = MapPoint { input_bin: b as f32, freq_grad: 1.0 };
            }
            return;
        };
        let bottom_offset = first.input - first.output;
        for b in 0..bands.min(first.output.ceil() as i64).max(0) {
            self.output_map[b as usize] = MapPoint { input_bin: b as f32 + bottom_offset, freq_grad: 1.0 };
        }
        for pair in self.peaks.windows(2) {
            let (prev, next) = (pair[0], pair[1]);
            let range_scale = 1.0 / (next.output - prev.output);
            let out_offset = prev.input - prev.output;
            let out_scale = next.input - next.output - prev.input + prev.output;
            let grad_scale = out_scale * range_scale;
            let start = (prev.output.ceil() as i64).max(0);
            let end = bands.min(next.output.ceil() as i64);
            for b in start..end {
                let r = (b as f32 - prev.output) * range_scale;
                let h = r * r * (3.0 - 2.0 * r);
                let out_b = b as f32 + out_offset + h * out_scale;
                let grad_h = 6.0 * r * (1.0 - r);
                let grad_b = 1.0 + grad_h * grad_scale;
                self.output_map[b as usize] = MapPoint { input_bin: out_b, freq_grad: grad_b };
            }
        }
        let top_offset = last.input - last.output;
        for b in (last.output as i64).max(0)..bands {
            self.output_map[b as usize] = MapPoint { input_bin: b as f32 + top_offset, freq_grad: 1.0 };
        }
    }

    fn inv_map_formant(&self, freq: f32) -> f32 {
        if freq * self.inv_formant_multiplier > self.freq_tonality_limit {
            freq + (1.0 - self.formant_multiplier) * self.freq_tonality_limit
        } else {
            freq * self.inv_formant_multiplier
        }
    }

    /// A very rough guess at the lowest note, from the three highest peaks, smoothed over windows.
    fn estimate_frequency(&mut self) -> f32 {
        let metric = &self.formant_metric;
        let mut peak_indices = [0usize; 3];
        for b in 1..self.bands.saturating_sub(1) {
            let e = metric[b];
            if e < metric[b - 1] || e <= metric[b + 1] {
                continue;
            }
            if e > metric[peak_indices[0]] {
                if e > metric[peak_indices[1]] {
                    if e > metric[peak_indices[2]] {
                        peak_indices = [peak_indices[1], peak_indices[2], b];
                    } else {
                        peak_indices = [peak_indices[1], b, peak_indices[2]];
                    }
                } else {
                    peak_indices[0] = b;
                }
            }
        }
        let mut estimate = peak_indices[2] as i64;
        if metric[peak_indices[1]] > metric[peak_indices[2]] * 0.1 {
            let diff = (estimate - peak_indices[1] as i64).abs();
            if diff > estimate / 8 && diff < estimate * 7 / 8 {
                estimate %= diff;
            }
            if metric[peak_indices[0]] > metric[peak_indices[2]] * 0.01 {
                let diff = (estimate - peak_indices[0] as i64).abs();
                if diff > estimate / 8 && diff < estimate * 7 / 8 {
                    estimate %= diff;
                }
            }
        }
        let weight = metric[peak_indices[2]];
        self.freq_estimate_weighted += (estimate as f32 * weight - self.freq_estimate_weighted) * 0.25;
        self.freq_estimate_weight += (weight - self.freq_estimate_weight) * 0.25;
        self.freq_estimate_weighted / (self.freq_estimate_weight + 1e-30)
    }

    /// The sound's tone outline (its formants), then each band's energy moved to where the formants should be.
    fn update_formants(&mut self) {
        let bands = self.bands;
        self.formant_metric.fill(0.0);
        for c in 0..self.channels {
            for (b, bin) in self.channel_bands[c * bands..(c + 1) * bands].iter().enumerate() {
                self.formant_metric[b] += bin.input_energy;
            }
        }
        self.freq_estimate = self.stft.freq_to_band(self.formant_base_freq);
        if self.formant_base_freq <= 0.0 {
            self.freq_estimate = self.estimate_frequency();
        }

        // An outline over the peaks (as wide as the notes are apart), then one under that, to smooth it.
        let mut decay = 1.0 - 1.0 / (self.freq_estimate * 0.5 + 1.0);
        let mut e = 0.0f32;
        let metric = &mut self.formant_metric[..bands];
        for _ in 0..2 {
            for m in metric.iter_mut().rev() {
                e = cmax(*m, e * decay);
                *m = e;
            }
            for m in metric.iter_mut() {
                e = cmax(*m, e * decay);
                *m = e;
            }
        }
        decay = 1.0 / decay;
        for _ in 0..2 {
            for m in metric.iter_mut().rev() {
                e = cmin(*m, e * decay);
                *m = e;
            }
            for m in metric.iter_mut() {
                e = cmin(*m, e * decay);
                *m = e;
            }
        }

        for b in 0..bands {
            let input_f = self.stft.band_to_freq(b as f32);
            let output_f = if self.formant_compensation { self.map_freq(input_f) } else { input_f };
            let output_f = self.inv_map_formant(output_f);
            let input_e = self.formant_metric[b];
            let target_e = self.formant_at(self.stft.freq_to_band(output_f));
            let ratio = target_e / (input_e + 1e-30);
            for c in 0..self.channels {
                self.channel_bands[c * bands + b].input_energy *= ratio;
            }
        }
    }

    fn formant_at(&self, band: f32) -> f32 {
        if band < 0.0 {
            return 0.0;
        }
        let band = band.min(self.bands as f32);
        let floor = band.floor();
        let frac = band - floor;
        let floor = floor as usize;
        let low = self.formant_metric[floor];
        let high = self.formant_metric[floor + 1];
        low + (high - low) * frac
    }
}

fn polar(phase: f32) -> Complex32 {
    Complex32::new(phase.cos(), phase.sin())
}

/// `a` times `b`, written out as the original does (so the rounding is the same).
fn mul(a: Complex32, b: Complex32) -> Complex32 {
    Complex32::new(a.re * b.re - a.im * b.im, a.re * b.im + a.im * b.re)
}

/// `a` times `b` turned the other way (its conjugate), written out as the original does.
fn mul_conj(a: Complex32, b: Complex32) -> Complex32 {
    Complex32::new(b.re * a.re + b.im * a.im, b.re * a.im - b.im * a.re)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::to_length;

    const RATE: f32 = 44_100.0;

    fn sine(freq: f32, seconds: f32) -> Vec<f32> {
        (0..(RATE * seconds) as usize).map(|i| (2.0 * PI * freq * i as f32 / RATE).sin() * 0.5).collect()
    }

    /// The frequency of a steady tone, from how often it crosses zero (its middle half only: away from the ends).
    fn frequency(sound: &[f32]) -> f32 {
        let middle = &sound[sound.len() / 4..sound.len() * 3 / 4];
        let ups = middle.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        ups as f32 * RATE / middle.len() as f32
    }

    fn rms(sound: &[f32]) -> f32 {
        let middle = &sound[sound.len() / 4..sound.len() * 3 / 4];
        (middle.iter().map(|s| s * s).sum::<f32>() / middle.len() as f32).sqrt()
    }

    #[test]
    fn a_stretched_tone_keeps_its_pitch_and_loudness() {
        let tone = sine(440.0, 2.0);
        for ratio in [0.6f32, 1.0, 1.5, 2.0, 3.0] {
            let length = (tone.len() as f32 * ratio) as usize;
            let out = to_length(&[&tone], RATE, length, 0.0);
            assert_eq!(out[0].len(), length);
            let f = frequency(&out[0]);
            assert!((f - 440.0).abs() < 3.0, "{ratio}x: {f} Hz");
            let loudness = rms(&out[0]) / rms(&tone);
            assert!((0.85..1.15).contains(&loudness), "{ratio}x: loudness {loudness}");
            assert!(out[0].iter().all(|s| s.is_finite()));
        }
    }

    #[test]
    fn a_moved_tone_moves_by_the_semitones_asked() {
        let tone = sine(330.0, 2.0);
        for (semitones, want) in [(12.0f32, 660.0f32), (-12.0, 165.0), (7.0, 330.0 * 1.4983)] {
            let out = to_length(&[&tone], RATE, tone.len(), semitones);
            let f = frequency(&out[0]);
            assert!((f - want).abs() < want * 0.01, "{semitones} semitones: {f} Hz, wanted {want}");
        }
    }

    #[test]
    fn the_same_sound_on_both_sides_stays_the_same() {
        let mut noisy = sine(200.0, 1.5);
        for (i, s) in noisy.iter_mut().enumerate() {
            *s += ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.2;
        }
        let out = to_length(&[&noisy, &noisy], RATE, noisy.len() * 5 / 4, 2.0);
        // Not to the last bit: the second side is locked to the first through a few sums, which round. The original
        // C++ differs by 1.6e-4 here too.
        let worst = out[0].iter().zip(&out[1]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-3, "the sides differ by {worst}");
    }

    #[test]
    fn silence_stays_silent_and_too_short_is_said() {
        let silence = vec![0.0f32; 44_100];
        let out = to_length(&[&silence], RATE, 60_000, 3.0);
        assert!(out[0].iter().all(|&s| s == 0.0));

        let short = sine(440.0, 0.05);
        let mut stretch = Stretch::new(1, RATE);
        let mut out = vec![1.0f32; 4000];
        assert!(!stretch.exact(&[&short], short.len(), &mut [&mut out], 4000));
        assert!(out.iter().all(|&s| s == 0.0), "a sound too short comes back silent");
    }

    /// Past 2x the phases get a little chance; with it, and squashed to a quarter, nothing goes wrong.
    #[test]
    fn far_stretches_stay_finite_and_the_same_every_run() {
        let mut sound = sine(110.0, 1.0);
        for (i, s) in sound.iter_mut().enumerate() {
            if i % 11_025 < 200 {
                *s += 0.8 * ((i * 31 % 17) as f32 / 8.0 - 1.0);
            }
        }
        for length in [sound.len() / 4, sound.len() * 4] {
            let a = to_length(&[&sound], RATE, length, 0.0);
            let b = to_length(&[&sound], RATE, length, 0.0);
            assert!(a[0].iter().all(|s| s.is_finite() && s.abs() < 4.0));
            assert_eq!(a, b);
        }
    }

    /// Fed a piece at a time, as a player would, it gives what a whole-sound stretch gives once it's started.
    #[test]
    fn piece_by_piece_runs_steadily() {
        let tone = sine(523.25, 3.0);
        let mut stretch = Stretch::new(1, RATE);
        let mut out = Vec::new();
        let mut buffer = vec![0.0f32; 512];
        for piece in tone.chunks(400) {
            let mut outs = [buffer.as_mut_slice()];
            stretch.process(&[piece], piece.len(), &mut outs, 512);
            out.extend_from_slice(&buffer);
        }
        let f = frequency(&out[out.len() / 4..]);
        assert!((f - 523.25).abs() < 4.0, "{f} Hz");
        assert!(out.iter().all(|s| s.is_finite()));
    }
}
