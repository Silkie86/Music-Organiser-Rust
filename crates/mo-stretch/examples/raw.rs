//! Stretches a raw sound file the way `tools/stretchtwin/twin.cpp` does with the original C++, so the two can be
//! compared (`tools/stretchtwin/run.sh`). Not part of the app.
//!
//! `raw IN CHANNELS RATE IN_SAMPLES OUT_SAMPLES SEMITONES FORMANT_SEMITONES COMPENSATE OUT`: IN and OUT are 32-bit
//! floats, little-endian, one channel after another.

use std::io::Write;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, channels, rate, in_samples, out_samples, semitones, formant, compensate, output] = args.as_slice()
    else {
        return Err("raw IN CHANNELS RATE IN_SAMPLES OUT_SAMPLES SEMITONES FORMANT_SEMITONES COMPENSATE OUT".into());
    };
    let number = |s: &String| s.parse::<f64>().map_err(|e| format!("{s}: {e}"));
    let channels = number(channels)? as usize;
    let rate = number(rate)? as f32;
    let in_samples = number(in_samples)? as usize;
    let out_samples = number(out_samples)? as usize;
    let semitones = number(semitones)? as f32;
    let formant = number(formant)? as f32;
    let compensate = compensate == "1";
    if channels == 0 || in_samples == 0 {
        return Err("CHANNELS and IN_SAMPLES must be at least 1".into());
    }

    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let all: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    if all.len() < channels * in_samples {
        return Err(format!("{input} is shorter than {channels} x {in_samples} samples"));
    }
    let inputs: Vec<&[f32]> = all.chunks_exact(in_samples).take(channels).collect();
    let mut out = vec![vec![0.0f32; out_samples]; channels];
    let mut stretch = mo_stretch::Stretch::new(channels, rate);
    if semitones != 0.0 {
        stretch.set_transpose_semitones(semitones, 0.0);
    }
    if formant != 0.0 || compensate {
        stretch.set_formant_semitones(formant, compensate);
    }
    let mut outputs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
    stretch.exact(&inputs, in_samples, &mut outputs, out_samples);

    let mut file = std::fs::File::create(output).map_err(|e| format!("{output}: {e}"))?;
    for channel in &out {
        let bytes: Vec<u8> = channel.iter().flat_map(|v| v.to_le_bytes()).collect();
        file.write_all(&bytes).map_err(|e| format!("{output}: {e}"))?;
    }
    Ok(())
}
