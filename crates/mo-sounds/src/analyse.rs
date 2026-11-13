//! Measures a sound: loudness and envelope, timbre (MFCCs and spectral shape), pitch (YIN), the notes in it (chroma),
//! loop or one-shot and tempo, character words and the feature vector the galaxy and "similar sounds" use.
//!
//! A port of the C# app's `Sounds/SoundAnalyzer.cs`, step for step (down to where C# adds in single precision), so a
//! sound analysed here matches the one the C# app stored. Finding the key and chords from the audio is left to the
//! harmony engine (phase 6): until then a key comes from the file name or the root note (see DIFFERENCES.md).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use mo_audio::dsp::{self, MelFilters, MelScale, hz_to_midi};
use mo_audio::{AudioData, AudioError};

use crate::analysis::{Peaks, SoundAnalysis};
use crate::types;

const N_FFT: usize = 2048;
const HOP: usize = 512;
const MELS: usize = 40;
const MFCCS: usize = 20;
const CHROMA_FFT: usize = 8192;

/// The names of the entries in [`Analysed::features`], in order (as C# `SoundAnalyzer.FeatureNames`).
pub fn feature_names() -> Vec<String> {
    let mut n: Vec<String> = (0..MFCCS).map(|i| format!("mfcc{i}")).collect();
    n.extend((0..MFCCS).map(|i| format!("mfcc{i}_sd")));
    n.extend(
        [
            "centroid",
            "centroid_sd",
            "bandwidth",
            "rolloff",
            "flatness",
            "flux",
            "zcr",
            "log_seconds",
            "log_attack",
            "log_decay",
            "crest",
            "rms_db",
            "band_sub",
            "band_low",
            "band_lowmid",
            "band_high",
            "band_air",
            "pitch_conf",
            "tonal",
            "width",
            "onset_rate",
        ]
        .map(str::to_owned),
    );
    n.extend((0..12).map(|i| format!("chroma{i}")));
    n
}

/// A sound's analysis, and its feature vector (kept apart: the database stores the vector in its own column).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Analysed {
    pub analysis: SoundAnalysis,
    pub features: Vec<f32>,
}

/// Reads up to a minute of the file at `path` and analyses it.
pub fn analyse_file(path: &Path) -> Result<Analysed, AudioError> {
    let audio = mo_audio::read(path, mo_audio::MAX_SECONDS)?;
    Ok(analyse(&audio, &path.to_string_lossy()))
}

/// Rounds as C#'s `Math.Round(value, digits)`: halves to even, by scaling by a power of ten.
pub(crate) fn round_to(v: f64, digits: i32) -> f64 {
    if v.abs() >= 1e16 {
        return v;
    }
    let p = 10f64.powi(digits);
    (v * p).round_ties_even() / p
}

fn db(v: f64) -> f64 {
    20.0 * v.max(1e-6).log10()
}

/// The sum of single-precision numbers as .NET's `Enumerable.Sum` gives it: added in double, handed back as single.
fn sum_f32(s: &[f32]) -> f32 {
    s.iter().map(|v| f64::from(*v)).sum::<f64>() as f32
}

fn mean(v: impl ExactSizeIterator<Item = f64>) -> f64 {
    let n = v.len();
    if n == 0 { f64::NAN } else { v.sum::<f64>() / n as f64 }
}

/// Analyses decoded audio. `path` is only read for what the name says (loop or one-shot, tempo, key).
pub fn analyse(audio: &AudioData, path: &str) -> Analysed {
    let sr_u = audio.sample_rate;
    let sr = sr_u as usize;
    let x = audio.mono();
    let x: &[f32] = &x;
    let mut a = SoundAnalysis {
        seconds: round_to(if audio.file_seconds > 0.0 { audio.file_seconds } else { audio.seconds() }, 3),
        sample_rate: i32::try_from(sr_u).unwrap_or(i32::MAX),
        channel_count: i32::try_from(audio.channels.len()).unwrap_or(i32::MAX),
        width: stereo_width(audio),
        ..SoundAnalysis::default()
    };

    // ---- level and envelope ----
    let (mut peak, mut sum_sq) = (0f64, 0f64);
    for &v in x {
        let av = f64::from(v.abs());
        if av > peak {
            peak = av;
        }
        sum_sq += f64::from(v * v);
    }
    let rms = if x.is_empty() { 0.0 } else { (sum_sq / x.len() as f64).sqrt() };
    a.peak_db = round_to(db(peak), 1);
    a.rms_db = round_to(db(rms), 1);
    a.waveform = Peaks::new(&waveform_peaks(x, 160, peak));

    let env = envelope(x, sr);
    let env_peak = env.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let env_peak_idx = env.iter().position(|v| *v == env_peak).unwrap_or(0);
    let env_peak = env.get(env_peak_idx).copied().unwrap_or(0.0);
    let mut start_idx = 0;
    // Leading silence doesn't count toward the attack.
    while start_idx < env_peak_idx && env[start_idx] < env_peak * 0.05 {
        start_idx += 1;
    }
    a.attack_ms = round_to((env_peak_idx - start_idx) as f64 * 5.0, 1);
    let mut decay_idx = env_peak_idx;
    // Until it has fallen 30 dB.
    while decay_idx < env.len() && env[decay_idx] > env_peak * 0.0316 {
        decay_idx += 1;
    }
    a.decay_ms = round_to((decay_idx - env_peak_idx) as f64 * 5.0, 1);

    // ---- spectrum: timbre is measured on the first 10 seconds, plenty, and it keeps scans quick ----
    let timbre = &x[..x.len().min(sr * 10)];
    let spec = dsp::power_spectrogram(timbre, N_FFT, HOP, true);
    let bins = N_FFT / 2 + 1;
    let bin_hz = f64::from(sr_u) / N_FFT as f64;
    let mel = mel_bank(sr_u).apply(&spec);

    // Only frames with sound in them describe the timbre.
    let frame_energy: Vec<f32> = spec.iter().map(|f| sum_f32(f)).collect();
    let max_e = frame_energy.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let max_e = if frame_energy.is_empty() { 0.0 } else { f64::from(max_e) };
    let mut active: Vec<usize> = (0..spec.len()).filter(|&i| f64::from(frame_energy[i]) > max_e * 1e-4).collect();
    if active.is_empty() {
        active = (0..spec.len()).collect();
    }

    let mfcc: Vec<[f64; MFCCS]> = active.iter().map(|&i| mfcc_of(&mel[i])).collect();
    let mut centroids = Vec::with_capacity(active.len());
    let (mut bandwidth_sum, mut rolloff_sum, mut flat_sum, mut flux_sum) = (0.0, 0.0, 0.0, 0.0);
    let mut bands = [0f64; 5];
    // Two buffers of magnitudes, this frame's and the last one's, swapped each frame.
    let mut mag = vec![0f32; bins];
    let mut prev_mag = vec![0f32; bins];
    let mut first = true;
    for &i in &active {
        let f = &spec[i];
        let (mut total, mut weighted) = (0f64, 0f64);
        for (k, &p) in f.iter().enumerate().take(bins).skip(1) {
            total += f64::from(p);
            // C# multiplies the single-precision power by the bin number in single precision first.
            weighted += f64::from(p * k as f32) * bin_hz;
        }
        let c = if total > 0.0 { weighted / total } else { 0.0 };
        centroids.push(c);
        let mut bw = 0.0;
        for (k, &p) in f.iter().enumerate().take(bins).skip(1) {
            let d = k as f64 * bin_hz - c;
            bw += f64::from(p) * d * d;
        }
        bandwidth_sum += if total > 0.0 { (bw / total).sqrt() } else { 0.0 };
        let mut cum = 0.0;
        let mut roll = 0.0;
        for (k, &p) in f.iter().enumerate().take(bins).skip(1) {
            cum += f64::from(p);
            if cum >= 0.85 * total {
                roll = k as f64 * bin_hz;
                break;
            }
        }
        rolloff_sum += roll;
        let (mut log_sum, mut lin_sum) = (0f64, 0f64);
        for &p in f.iter().take(bins).skip(1) {
            log_sum += (f64::from(p) + 1e-12).ln();
            lin_sum += f64::from(p);
        }
        let m = (bins - 1) as f64;
        flat_sum += if lin_sum > 0.0 { (log_sum / m).exp() / (lin_sum / m) } else { 0.0 };
        for (m, p) in mag.iter_mut().zip(f) {
            *m = f64::from(*p).sqrt() as f32;
        }
        if !first {
            let mut fl = 0f64;
            for (m, pm) in mag.iter().zip(&prev_mag) {
                fl += f64::from((m - pm).max(0.0));
            }
            flux_sum += fl / 1e-9f64.max(f64::from(sum_f32(&mag)));
        }
        first = false;
        std::mem::swap(&mut mag, &mut prev_mag);
        for (k, &p) in f.iter().enumerate().take(bins).skip(1) {
            let hz = k as f64 * bin_hz;
            let b = if hz < 60.0 {
                0
            } else if hz < 250.0 {
                1
            } else if hz < 2000.0 {
                2
            } else if hz < 6000.0 {
                3
            } else {
                4
            };
            bands[b] += f64::from(p);
        }
    }
    let n = active.len() as f64;
    let centroid = mean(centroids.iter().copied());
    let centroid_sd = mean(centroids.iter().map(|c| (c - centroid) * (c - centroid))).sqrt();
    let flatness = flat_sum / n;
    let band_total = bands.iter().sum::<f64>().max(1e-12);
    a.brightness = centroid.round_ties_even();
    a.noisiness = round_to((flatness * 3.0).clamp(0.0, 1.0), 3);

    // ---- pitch and the notes in it ----
    let (pitch, pitch_conf) = yin(x, sr, start_idx * sr / 200 + sr / 50, 30.0, 2000.0);
    a.pitch_confidence = round_to(pitch_conf, 3);
    if let Some(f0) = pitch
        && pitch_conf >= 0.6
    {
        a.pitch = Some(round_to(hz_to_midi(f0), 2));
    }
    if let Some(root) = audio.embedded_root_note {
        a.pitch = Some(f64::from(root));
        a.pitch_confidence = 1.0;
    }

    let chroma_frames = chroma_frames(&x[..x.len().min(sr * 30)], sr_u);
    let mut chroma = [0f64; 12];
    for c in &chroma_frames {
        for (s, v) in chroma.iter_mut().zip(c) {
            *s += v;
        }
    }
    let chroma_max = chroma.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if chroma_max > 0.0 {
        for c in &mut chroma {
            *c /= chroma_max;
        }
    }
    a.chroma = chroma.iter().map(|c| round_to(*c, 3)).collect();
    let tonal = 1.0 - (flatness * 3.0).clamp(0.0, 1.0);

    // ---- loop and tempo ----
    let onset = onset_envelope(&mel);
    let frame_rate = f64::from(sr_u) / HOP as f64;
    let onset_count = count_peaks(&onset);
    let onset_rate = if timbre.is_empty() { 0.0 } else { onset_count as f64 / (timbre.len() as f64 / f64::from(sr_u)) };
    a.is_loop = audio
        .embedded_is_loop
        .or_else(|| types::loop_from_path(path))
        .unwrap_or_else(|| a.seconds >= 1.5 && onset_count >= 3 && bar_fit(a.seconds, None).is_some());
    if a.is_loop {
        if let Some(et) = audio.embedded_tempo {
            (a.bpm, a.bpm_source) = (Some(round_to(et, 2)), "file tag".into());
        } else if let Some(nt) = types::tempo_from_name(path) {
            (a.bpm, a.bpm_source) = (Some(nt), "file name".into());
        } else if let Some(bt) = estimate_tempo(&onset, frame_rate, a.seconds) {
            (a.bpm, a.bpm_source) = (Some(bt), "detected".into());
        }
        // A tag or a tempo heard that doesn't fit the loop's length gives way to a written one that does.
        check_loop_tempo(&mut a, path);
    }

    // Key: written in the name, else (for a one-shot) its root note. Finding it in the audio waits for the harmony
    // engine, as do the chords.
    if let Some((tonic, minor)) = types::key_from_name(path).filter(|_| tonal > 0.3) {
        (a.key_tonic, a.key_minor, a.key_confidence, a.key_source) = (Some(tonic), minor, 1.0, "file name".into());
    } else if let Some(p) = a.pitch.filter(|_| !a.is_loop) {
        let tonic = (p.round_ties_even() as i32).rem_euclid(12);
        (a.key_tonic, a.key_minor, a.key_confidence, a.key_source) =
            (Some(tonic), false, a.pitch_confidence, "root note".into());
    }

    // ---- character words ----
    let ch = &mut a.character;
    if a.attack_ms <= 10.0 && peak / rms.max(1e-9) > 4.0 {
        ch.push("punchy".into());
    } else if a.attack_ms >= 80.0 {
        ch.push("soft attack".into());
    }
    ch.push(
        if centroid < 700.0 {
            "dark"
        } else if centroid > 3500.0 {
            "bright"
        } else {
            "warm"
        }
        .into(),
    );
    if bands[0] / band_total > 0.25 || bands[1] / band_total > 0.5 {
        ch.push("deep".into());
    }
    if bands[4] / band_total > 0.2 {
        ch.push("airy".into());
    }
    if tonal > 0.7 && pitch_conf > 0.7 {
        ch.push("tonal".into());
    } else if flatness > 0.25 {
        ch.push("noisy".into());
    }
    if a.width > 0.35 {
        ch.push("wide".into());
    } else if a.channel_count == 1 || a.width < 0.02 {
        ch.push("mono".into());
    }
    if !a.is_loop {
        ch.push(
            if a.decay_ms < 150.0 {
                "short"
            } else if a.decay_ms > 1200.0 {
                "long"
            } else {
                "medium"
            }
            .into(),
        );
    }

    // ---- feature vector ----
    let mut feats: Vec<f64> = (0..MFCCS).map(|i| mean(mfcc.iter().map(|m| m[i]))).collect();
    for i in 0..MFCCS {
        let mu = feats[i];
        feats.push(mean(mfcc.iter().map(|m| (m[i] - mu) * (m[i] - mu))).sqrt());
    }
    feats.extend([
        centroid / 1000.0,
        centroid_sd / 1000.0,
        bandwidth_sum / n / 1000.0,
        rolloff_sum / n / 1000.0,
        flatness,
        flux_sum / (n - 1.0).max(1.0),
        zcr(timbre),
        (a.seconds + 0.01).ln(),
        (a.attack_ms + 1.0).ln(),
        (a.decay_ms + 1.0).ln(),
        db(peak) - db(rms),
        db(rms) / 10.0,
        bands[0] / band_total,
        bands[1] / band_total,
        bands[2] / band_total,
        bands[3] / band_total,
        bands[4] / band_total,
        pitch_conf,
        tonal,
        a.width,
        (onset_rate + 1.0).ln(),
    ]);
    feats.extend(chroma);
    let features = feats.iter().map(|f| if f.is_finite() { *f as f32 } else { 0.0 }).collect();
    Analysed { analysis: a, features }
}

/// The mel filters for a sample rate, made once and kept.
fn mel_bank(sr: u32) -> Arc<MelFilters> {
    static BANKS: OnceLock<Mutex<HashMap<u32, Arc<MelFilters>>>> = OnceLock::new();
    let mut banks = BANKS.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(banks.entry(sr).or_insert_with(|| {
        Arc::new(MelFilters::new(
            N_FFT / 2 + 1,
            MELS,
            20.0,
            (f64::from(sr) / 2.0).min(16_000.0),
            sr,
            MelScale::Slaney,
            true,
        ))
    }))
}

/// MFCCs: an orthonormal DCT-II of the log-mel frame, scaled down to keep the features comparable.
fn mfcc_of(mel_frame: &[f32]) -> [f64; MFCCS] {
    let mut log_mel = [0f64; MELS];
    for (l, v) in log_mel.iter_mut().zip(mel_frame) {
        *l = 10.0 * f64::from(*v).max(1e-10).log10();
    }
    let mut c = [0f64; MFCCS];
    for (k, ck) in c.iter_mut().enumerate() {
        let mut s = 0.0;
        for (m, l) in log_mel.iter().enumerate() {
            s += l * (std::f64::consts::PI * k as f64 * (m as f64 + 0.5) / MELS as f64).cos();
        }
        *ck = s * ((if k == 0 { 1.0 } else { 2.0 }) / MELS as f64).sqrt() / 10.0;
    }
    c
}

fn zcr(x: &[f32]) -> f64 {
    if x.len() < 2 {
        return 0.0;
    }
    let z = x.windows(2).filter(|w| (w[1] >= 0.0) != (w[0] >= 0.0)).count();
    z as f64 / x.len() as f64 * 10.0
}

fn stereo_width(audio: &AudioData) -> f64 {
    let [l, r, ..] = audio.channels.as_slice() else { return 0.0 };
    let (mut mid, mut side) = (0f64, 0f64);
    for (l, r) in l.iter().zip(r) {
        // C# adds the two in single precision, then halves in double.
        let m = f64::from(l + r) * 0.5;
        let s = f64::from(l - r) * 0.5;
        mid += m * m;
        side += s * s;
    }
    if mid + side <= 0.0 { 0.0 } else { round_to((side / (mid + side)).sqrt() * 2f64.sqrt(), 3) }
}

fn waveform_peaks(x: &[f32], points: usize, peak: f64) -> Vec<f32> {
    let mut w = vec![0f32; points];
    if x.is_empty() || peak <= 0.0 {
        return w;
    }
    for (p, out) in w.iter_mut().enumerate() {
        let from = x.len() * p / points;
        let to = (from + 1).max(x.len() * (p + 1) / points);
        let m = x.get(from..to.min(x.len())).unwrap_or_default().iter().fold(0f32, |m, v| m.max(v.abs()));
        *out = round_to(f64::from(m) / peak, 3) as f32;
    }
    w
}

/// RMS level in 5 ms frames.
fn envelope(x: &[f32], sr: usize) -> Vec<f64> {
    let size = (sr / 200).max(1);
    x.chunks(size).map(|c| (c.iter().map(|v| f64::from(v * v)).sum::<f64>() / size as f64).sqrt()).collect()
}

/// Per-frame 12-note chroma (`dsp::chroma_of`, with a long FFT so low notes resolve). Silent frames are left empty.
fn chroma_frames(x: &[f32], sr: u32) -> Vec<[f64; 12]> {
    let hop = CHROMA_FFT / 2;
    let spec = dsp::power_spectrogram(x, CHROMA_FFT, hop, true);
    let bin_hz = f64::from(sr) / CHROMA_FFT as f64;
    let energies: Vec<f32> = spec.iter().map(|f| sum_f32(f)).collect();
    let max_e = f64::from(energies.iter().copied().fold(0f32, f32::max));
    let mut frames = Vec::with_capacity(spec.len());
    let mut mag = vec![0f64; CHROMA_FFT / 2 + 1];
    for (f, &e) in spec.iter().zip(&energies) {
        let c = if f64::from(e) >= max_e * 1e-3 { dsp::chroma_of(f, bin_hz, &mut mag) } else { [0f64; 12] };
        frames.push(c);
    }
    frames
}

/// Spectral flux of the log-mel spectrogram: a standard onset strength curve.
fn onset_envelope(mel: &[Vec<f32>]) -> Vec<f64> {
    let mut o = vec![0f64; mel.len()];
    for f in 1..mel.len() {
        o[f] = mel[f]
            .iter()
            .zip(&mel[f - 1])
            .take(MELS)
            .map(|(now, before)| (f64::from(*now) + 1e-8).log10() - (f64::from(*before) + 1e-8).log10())
            .filter(|d| *d > 0.0)
            .sum();
    }
    o
}

fn count_peaks(onset: &[f64]) -> usize {
    if onset.len() < 3 {
        return 0;
    }
    let max = onset.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let avg = onset.iter().sum::<f64>() / onset.len() as f64;
    let threshold = avg + 0.3 * (max - avg);
    let mut count = 0;
    let mut last: isize = -10;
    for i in 1..onset.len() - 1 {
        if onset[i] > threshold && onset[i] >= onset[i - 1] && onset[i] > onset[i + 1] && i as isize - last > 4 {
            count += 1;
            last = i as isize;
        }
    }
    count
}

/// How far a loop `seconds` long at `bpm` is from a whole number of bars of four beats (at least one), in beats.
fn off_bars(seconds: f64, bpm: f64) -> f64 {
    let beats = seconds * bpm / 60.0;
    let whole = ((beats / 4.0).round() * 4.0).max(4.0);
    (beats - whole).abs()
}

/// How near a whole number of bars a loop must be for its tempo to fit: a tenth of a beat (a loop's tail or a
/// few milliseconds cut aside).
const FITS_BARS: f64 = 0.1;

/// The tempo to trust for a loop `seconds` long at `path`, given the one `found` (from the file's tag, its name or
/// heard): the tempo written in its name, else in its folder's ("Flow 162.wav", "Loops 128BPM"), when that one fits
/// the loop's length in whole bars and the found one doesn't. A file's tag often says 120 whatever the loop is, and a
/// tempo heard can be off; a pack's written tempo with a length that agrees is surer (Solomon, 13 Nov: "Flow 162" at
/// 120, a 1.9 s loop in "VEE2 Loops 128BPM" at 86).
pub fn checked_tempo(seconds: f64, found: Option<f64>, path: &str) -> Option<f64> {
    let written = types::tempo_from_name(path).or_else(|| types::tempo_from_folders(path));
    let (Some(written), true) = (written, seconds.is_finite() && seconds > 0.0) else { return found };
    let fits = |bpm: f64| off_bars(seconds, bpm) <= FITS_BARS;
    match found {
        Some(bpm) if fits(bpm) || !fits(written) => Some(bpm),
        _ if fits(written) => Some(written),
        other => other,
    }
}

/// A loop's tempo put right by [`checked_tempo`], saying where the new one is from: what reading the loop at `path`
/// again would give it. The analyser does it as it reads a sound; a list does it for the sounds analysed before the
/// check came in (13 Nov), whose kept tempo can be a tag's or a heard one that doesn't fit, so they show and filter by
/// the same tempo a new read would give, with nothing read again. Anything but a loop is left as it is.
pub fn check_loop_tempo(a: &mut SoundAnalysis, path: &str) {
    if !a.is_loop {
        return;
    }
    if let Some(written) = checked_tempo(a.seconds, a.bpm, path).filter(|w| Some(*w) != a.bpm) {
        let from = if types::tempo_from_name(path).is_some() { "file name" } else { "folder name" };
        (a.bpm, a.bpm_source) = (Some(written), from.into());
    }
}

/// When a loop is a whole number of bars at a sensible tempo, the tempo that fits (sample packs cut loops exactly).
/// With a `hint` (the tempo heard in the onsets), the fit closest to it, if one is close.
pub fn bar_fit(seconds: f64, hint: Option<f64>) -> Option<f64> {
    let mut best = None;
    let mut best_err = f64::MAX;
    for beats in [4.0, 8.0, 16.0, 32.0, 64.0, 2.0, 12.0, 24.0] {
        let bpm = beats * 60.0 / seconds;
        if !(70.0..=180.0).contains(&bpm) {
            continue;
        }
        let rounded = bpm.round_ties_even();
        let grid_err = (bpm - rounded).abs() / bpm;
        if grid_err >= 0.004 {
            continue;
        }
        let err = match hint {
            Some(h) => (bpm - h).abs().min((bpm - 2.0 * h).abs()) / h,
            None => grid_err,
        };
        if hint.is_some() && err > 0.04 {
            continue;
        }
        if err < best_err {
            best_err = err;
            best = Some(rounded);
        }
    }
    best
}

/// Tempo from the autocorrelation of the onset curve, snapped to a whole-bar fit of the loop's length when one agrees.
fn estimate_tempo(onset: &[f64], frame_rate: f64, seconds: f64) -> Option<f64> {
    if onset.len() < 16 {
        return bar_fit(seconds, None);
    }
    let avg = onset.iter().sum::<f64>() / onset.len() as f64;
    let o: Vec<f64> = onset.iter().map(|v| v - avg).collect();
    let mut best_bpm = None;
    let mut best_score = f64::MIN;
    // 60 to 200 in half steps, counted as C# counts them (adding 0.5 each time).
    let mut bpm = 60.0;
    while bpm <= 200.0 {
        let lag = frame_rate * 60.0 / bpm;
        // The beat, two beats and half a beat all count, so the real pulse wins.
        let mut score = 0.0;
        for (mult, w) in [(1.0, 1.0), (2.0, 0.5), (0.5, 0.25)] {
            score += w * autocorr(&o, lag * mult);
        }
        // A gentle pull toward 120.
        score *= (-0.5 * ((bpm / 120.0f64).log2() / 0.9).powi(2)).exp();
        if score > best_score {
            best_score = score;
            best_bpm = Some(bpm);
        }
        bpm += 0.5;
    }
    let Some(found) = best_bpm else { return bar_fit(seconds, None) };
    bar_fit(seconds, Some(found)).or(Some(found.round_ties_even()))
}

fn autocorr(o: &[f64], lag: f64) -> f64 {
    let l0 = lag.floor() as usize;
    let frac = lag - l0 as f64;
    let mut s = 0.0;
    let mut i = 0;
    while i + l0 + 1 < o.len() {
        s += o[i] * (o[i + l0] * (1.0 - frac) + o[i + l0 + 1] * frac);
        i += 1;
    }
    s / (o.len().saturating_sub(l0).max(1)) as f64
}

/// The YIN pitch detector (de Cheveigné and Kawahara), run over up to 8 frames from `start`, taking the median of the
/// sure ones. Gives the pitch in Hz (if found) and how sure it is, 0 to 1.
pub fn yin(x: &[f32], sr: usize, start: usize, f_min: f64, f_max: f64) -> (Option<f64>, f64) {
    let max_lag = (sr as f64 / f_min) as usize;
    let min_lag = ((sr as f64 / f_max) as usize).max(2);
    // A rate too low to hold the lowest pitch (only a damaged header says so): nothing to find.
    if max_lag <= min_lag {
        return (None, 0.0);
    }
    let w = (max_lag * 2).min(sr / 5);
    let mut results: Vec<(f64, f64)> = Vec::new();
    let mut d = vec![0f64; max_lag + 1];
    let mut cmnd = vec![0f64; max_lag + 1];
    for frame in 0..8 {
        let o = start + frame * w / 2;
        if o + w + max_lag >= x.len() {
            break;
        }
        for tau in 1..=max_lag {
            let mut s = 0f64;
            for i in 0..w {
                let diff = x[o + i] - x[o + i + tau];
                s += f64::from(diff * diff);
            }
            d[tau] = s;
        }
        // Cumulative mean normalised difference.
        cmnd[0] = 1.0;
        let mut run = 0.0;
        for tau in 1..=max_lag {
            run += d[tau];
            cmnd[tau] = if run <= 0.0 { 1.0 } else { d[tau] * tau as f64 / run };
        }
        let mut best = None;
        let mut tau = min_lag;
        while tau < max_lag {
            if cmnd[tau] < 0.15 {
                while tau + 1 < max_lag && cmnd[tau + 1] < cmnd[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        let best = best.unwrap_or_else(|| {
            let mut b = min_lag;
            for tau in min_lag..max_lag {
                if cmnd[tau] < cmnd[b] {
                    b = tau;
                }
            }
            b
        });
        // A parabola through the dip.
        let mut shift = 0.0;
        if best > 1 && best < max_lag {
            let (a, b, c) = (cmnd[best - 1], cmnd[best], cmnd[best + 1]);
            let den = a - 2.0 * b + c;
            if den.abs() > 1e-12 {
                shift = 0.5 * (a - c) / den;
            }
        }
        results.push((sr as f64 / (best as f64 + shift), 1.0 - cmnd[best].min(1.0)));
    }
    if results.is_empty() {
        return (None, 0.0);
    }
    let mut good: Vec<(f64, f64)> = results.iter().copied().filter(|r| r.1 > 0.5).collect();
    if good.is_empty() {
        return (None, results.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max));
    }
    // A stable sort, as C#'s OrderBy.
    good.sort_by(|a, b| a.0.total_cmp(&b.0));
    let median = good[good.len() / 2];
    let avg = good.iter().map(|r| r.1).sum::<f64>() / good.len() as f64;
    (Some(median.0), avg * good.len() as f64 / results.len() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(rate: u32, channels: Vec<Vec<f32>>) -> AudioData {
        AudioData { sample_rate: rate, channels, ..AudioData::default() }
    }

    fn sine(hz: f64, rate: u32, seconds: f64, level: f32) -> Vec<f32> {
        let n = (seconds * f64::from(rate)) as usize;
        (0..n).map(|i| level * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin() as f32).collect()
    }

    /// Steady noise from a fixed seed, so tests always see the same samples.
    fn noise(n: usize, level: f32) -> Vec<f32> {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                level * ((s >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0)
            })
            .collect()
    }

    #[test]
    fn a_written_tempo_that_fits_the_length_wins_over_one_that_doesnt() {
        // 16 beats at 162 BPM; a tag saying 120 doesn't fit.
        let flow = 16.0 * 60.0 / 162.0;
        assert_eq!(checked_tempo(flow, Some(120.0), r"D:\VOL 1\Straight Loops\Flow 162.wav"), Some(162.0));
        // A bar at 128 BPM, written in the folder's name; heard at 86.
        let bar = 4.0 * 60.0 / 128.0;
        assert_eq!(checked_tempo(bar, Some(86.0), r"D:\VEE2\VEE2 Loops 128BPM\VEE2 Electro Loop 028.wav"), Some(128.0));
        // The found one fits too: it stays (half or double time is the loop's own call).
        assert_eq!(checked_tempo(flow, Some(81.0), r"D:\Loops\Flow 162.wav"), Some(81.0));
        // A written tempo that doesn't fit the length changes nothing; nor does no written one.
        assert_eq!(checked_tempo(5.0, Some(93.0), r"D:\Loops\Flow 162.wav"), Some(93.0));
        assert_eq!(checked_tempo(flow, Some(120.0), r"D:\Loops\Flow.wav"), Some(120.0));
        // Nothing found: a written one that fits is taken.
        assert_eq!(checked_tempo(bar, None, r"D:\Loops 128 BPM\Hit.wav"), Some(128.0));
    }

    #[test]
    fn a_loop_read_before_the_check_gets_the_tempo_a_new_read_gives() {
        let flow = 16.0 * 60.0 / 162.0;
        let kept = |is_loop, bpm, source: &str| SoundAnalysis {
            seconds: flow,
            is_loop,
            bpm,
            bpm_source: source.into(),
            ..SoundAnalysis::default()
        };
        let mut a = kept(true, Some(120.0), "file tag");
        check_loop_tempo(&mut a, r"D:\VOL 1\Straight Loops\Flow 162.wav");
        assert_eq!((a.bpm, a.bpm_source.as_str()), (Some(162.0), "file name"));
        let mut a = kept(true, None, "");
        check_loop_tempo(&mut a, r"D:\Loops 162BPM\Flow.wav");
        assert_eq!((a.bpm, a.bpm_source.as_str()), (Some(162.0), "folder name"));
        // One that fits stays as it is, and so does anything but a loop.
        let mut a = kept(true, Some(81.0), "detected");
        check_loop_tempo(&mut a, r"D:\Loops\Flow 162.wav");
        assert_eq!((a.bpm, a.bpm_source.as_str()), (Some(81.0), "detected"));
        let mut a = kept(false, Some(120.0), "file tag");
        check_loop_tempo(&mut a, r"D:\Loops\Flow 162.wav");
        assert_eq!((a.bpm, a.bpm_source.as_str()), (Some(120.0), "file tag"));
    }

    #[test]
    fn the_features_match_their_names() {
        let a = analyse(&audio(44_100, vec![sine(440.0, 44_100, 1.0, 0.5)]), "tone.wav");
        assert_eq!(a.features.len(), feature_names().len());
        assert_eq!(feature_names().len(), 73);
        assert!(a.features.iter().all(|f| f.is_finite()));
    }

    #[test]
    fn a_held_a_is_found_as_a_tonal_one_shot_in_a() {
        let a = analyse(&audio(44_100, vec![sine(440.0, 44_100, 2.0, 0.5)]), "Synth/tone.wav").analysis;
        let pitch = a.pitch.unwrap();
        assert!((pitch - 69.0).abs() < 0.05, "{pitch}");
        assert!(a.pitch_confidence > 0.9);
        assert_eq!((a.key_tonic, a.key_minor, a.key_source.as_str()), (Some(9), false, "root note"));
        assert!(!a.is_loop);
        assert!(a.character.iter().any(|w| w == "tonal"), "{:?}", a.character);
        assert!(a.character.iter().any(|w| w == "mono"));
        assert!((a.peak_db - -6.0).abs() < 0.1);
        assert!((a.rms_db - -9.0).abs() < 0.1);
        let biggest = a.chroma.iter().copied().fold(0.0, f64::max);
        assert_eq!(a.chroma[9], biggest);
        assert!((a.brightness - 440.0).abs() < 20.0, "{}", a.brightness);
        assert_eq!(a.waveform.len(), 160);
    }

    #[test]
    fn noise_is_noisy_bright_and_has_no_pitch() {
        let a = analyse(&audio(44_100, vec![noise(44_100, 0.5)]), "hat.wav").analysis;
        assert!(a.noisiness > 0.5, "{}", a.noisiness);
        assert!(a.character.iter().any(|w| w == "noisy"), "{:?}", a.character);
        assert!(a.character.iter().any(|w| w == "bright"));
        assert_eq!(a.pitch, None);
        assert_eq!(a.key_tonic, None);
    }

    #[test]
    fn a_click_loop_cut_to_four_bars_is_a_loop_at_its_tempo() {
        // 120 BPM: a click every half second, 16 beats, 8 seconds.
        let rate = 22_050;
        let mut x = vec![0f32; rate as usize * 8];
        for beat in 0..16 {
            let at = beat * rate as usize / 2;
            for (i, s) in noise(400, 0.8).into_iter().enumerate() {
                x[at + i] = s * (1.0 - i as f32 / 400.0);
            }
        }
        let a = analyse(&audio(rate, vec![x]), "drums.wav").analysis;
        assert!(a.is_loop);
        assert_eq!((a.bpm, a.bpm_source.as_str()), (Some(120.0), "detected"));
        assert_eq!(bar_fit(8.0, None), Some(120.0));
    }

    #[test]
    fn notes_in_the_file_win() {
        let mut sound = audio(44_100, vec![sine(440.0, 44_100, 2.0, 0.5)]);
        sound.embedded_is_loop = Some(true);
        sound.embedded_tempo = Some(93.456);
        sound.embedded_root_note = Some(60);
        let a = analyse(&sound, "Loops/Cm Pad.wav").analysis;
        assert!(a.is_loop);
        assert_eq!((a.bpm, a.bpm_source.as_str()), (Some(93.46), "file tag"));
        assert_eq!((a.pitch, a.pitch_confidence), (Some(60.0), 1.0));
        assert_eq!((a.key_tonic, a.key_minor, a.key_source.as_str()), (Some(0), true, "file name"));
    }

    #[test]
    fn a_wide_stereo_sound_says_so() {
        let l = noise(22_050, 0.5);
        let r: Vec<f32> = l.iter().map(|v| -v).collect();
        let a = analyse(&audio(22_050, vec![l, r]), "x.wav").analysis;
        assert!((a.width - 2f64.sqrt()).abs() < 1e-3, "{}", a.width);
        assert!(a.character.iter().any(|w| w == "wide"));
    }

    #[test]
    fn silence_and_nothing_dont_break_it() {
        for x in [vec![], vec![0.0; 1000]] {
            let a = analyse(&audio(44_100, vec![x]), "x.wav");
            assert!(a.features.iter().all(|f| f.is_finite()));
            assert_eq!(a.analysis.pitch, None);
        }
        assert_eq!(analyse(&AudioData::default(), "x.wav").analysis.seconds, 0.0);
    }

    #[test]
    fn a_damaged_header_with_a_tiny_rate_doesnt_crash() {
        for rate in [0, 1, 8, 22, 59, 60, 61] {
            let a = analyse(&audio(rate, vec![noise(5000, 0.5)]), "x.wav");
            assert!(a.features.iter().all(|f| f.is_finite()), "{rate}");
        }
    }

    #[test]
    fn rounding_is_csharps() {
        assert_eq!(round_to(2.5, 0), 2.0);
        assert_eq!(round_to(0.1235, 3), 0.124);
        assert_eq!(round_to(-6.02, 1), -6.0);
    }
}
