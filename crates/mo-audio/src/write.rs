//! Writing 16- or 24-bit PCM WAV files (kit exports), as the C# `WavWriter` writes them.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::AudioError;

/// Writes `channels` (all the same length, samples from -1 to 1, louder ones clipped) to a new WAV file at `path`.
///
/// The file is written beside its final name first and then renamed, so a stopped or failed export never leaves half
/// a file, under the real name or beside it. `bits` is 16 or 24.
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
    let too_many = || AudioError::Format("Too many channels for a WAV file".into());
    let n = u16::try_from(channels.len()).map_err(|_| too_many())?;
    // Worked out in usize first: 21,846 or more 24-bit channels don't fit a WAV's block size.
    let block = u16::try_from(channels.len() * bytes).map_err(|_| too_many())?;
    let too_big = || AudioError::Format("Too long for a WAV file".into());
    let data_bytes = frames.checked_mul(usize::from(block)).ok_or_else(too_big)?;
    let data_size = u32::try_from(data_bytes).ok().filter(|n| *n <= u32::MAX - 37).ok_or_else(too_big)?;

    let mut head = Vec::with_capacity(44);
    head.extend_from_slice(b"RIFF");
    // RIFF chunks are padded to an even length; the pad counts in the file's size but not the data's.
    let pad = data_size & 1;
    head.extend_from_slice(&(36 + data_size + pad).to_le_bytes());
    head.extend_from_slice(b"WAVEfmt ");
    head.extend_from_slice(&16u32.to_le_bytes());
    head.extend_from_slice(&1u16.to_le_bytes());
    head.extend_from_slice(&n.to_le_bytes());
    head.extend_from_slice(&sample_rate.to_le_bytes());
    head.extend_from_slice(&sample_rate.wrapping_mul(u32::from(block)).to_le_bytes());
    head.extend_from_slice(&block.to_le_bytes());
    head.extend_from_slice(&bits.to_le_bytes());
    head.extend_from_slice(b"data");
    head.extend_from_slice(&data_size.to_le_bytes());

    let part = path.with_extension("wav.part");
    let written = write_part(&part, &head, channels, frames, bits).and_then(|()| std::fs::rename(&part, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(())
}

/// Streams the header and the samples to `part`, a little at a time, so a long export isn't held in memory twice.
fn write_part(part: &Path, head: &[u8], channels: &[Vec<f32>], frames: usize, bits: u16) -> std::io::Result<()> {
    let mut out = BufWriter::new(File::create(part)?);
    out.write_all(head)?;
    for i in 0..frames {
        for ch in channels {
            let v = ch.get(i).copied().unwrap_or(0.0).clamp(-1.0, 1.0);
            // C# rounds half to even (Math.Round).
            if bits == 16 {
                out.write_all(&((v * 32767.0).round_ties_even() as i16).to_le_bytes())?;
            } else {
                let x = (v * 8_388_607.0).round_ties_even() as i32;
                out.write_all(&x.to_le_bytes()[..3])?;
            }
        }
    }
    if (frames * channels.len() * usize::from(bits / 8)) % 2 == 1 {
        out.write_all(&[0])?;
    }
    out.into_inner().map_err(std::io::IntoInnerError::into_error)?.sync_all()
}
