//! Writing 16- or 24-bit PCM WAV files (kit exports), as the C# `WavWriter` writes them.

use std::path::Path;

use crate::AudioError;

/// Writes `channels` (all the same length, samples from -1 to 1, louder ones clipped) to a new WAV file at `path`.
///
/// The file is written beside its final name first and then renamed, so a stopped export never leaves half a file
/// under the real name. `bits` is 16 or 24.
pub fn write_wav(path: &Path, channels: &[Vec<f32>], sample_rate: u32, bits: u16) -> Result<(), AudioError> {
    if bits != 16 && bits != 24 {
        return Err(AudioError::NotSupported(format!("Writing {bits}-bit WAV files")));
    }
    let Some(frames) = channels.first().map(Vec::len) else {
        return Err(AudioError::Format("No channels to write".into()));
    };
    if channels.iter().any(|c| c.len() != frames) {
        return Err(AudioError::Format("The channels aren't all the same length".into()));
    }
    let bytes = usize::from(bits / 8);
    let too_big = || AudioError::Format("Too long for a WAV file".into());
    let data_bytes = frames.checked_mul(channels.len() * bytes).ok_or_else(too_big)?;
    let data_size = u32::try_from(data_bytes).ok().filter(|n| *n <= u32::MAX - 36).ok_or_else(too_big)?;
    let n = u16::try_from(channels.len()).map_err(|_| too_big())?;
    let block = n * (bits / 8);

    let mut out = Vec::with_capacity(44 + data_bytes);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_size).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&sample_rate.wrapping_mul(u32::from(block)).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_size.to_le_bytes());
    for i in 0..frames {
        for ch in channels {
            let v = ch.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
            // C# rounds half to even (Math.Round).
            if bits == 16 {
                out.extend_from_slice(&((v * 32767.0).round_ties_even() as i16).to_le_bytes());
            } else {
                let x = (v * 8_388_607.0).round_ties_even() as i32;
                out.extend_from_slice(&x.to_le_bytes()[..3]);
            }
        }
    }
    let part = path.with_extension("wav.part");
    std::fs::write(&part, &out)?;
    std::fs::rename(&part, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })?;
    Ok(())
}
