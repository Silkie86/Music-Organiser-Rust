//! Reading WAV and AIFF files, as the C# `AudioFile` reads them.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::{AudioData, AudioError};

/// The longest stretch decoded for analysis: long files are analysed from their first minute.
pub const MAX_SECONDS: f64 = 60.0;

/// The most bytes one read decodes, as .NET's largest array: past it a file is decoded only so far.
pub(crate) const MOST_BYTES: u64 = 0x7FFF_FFC7;

/// Whether the file's extension is one of `list` (ignoring case).
pub(crate) fn has_extension(path: &Path, list: &[&str]) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| list.iter().any(|w| w.eq_ignore_ascii_case(e)))
}

/// Whether [`read_from`] can start part way into this file: WAV and AIFF (by the extension, ignoring case).
pub fn can_read_from(path: &Path) -> bool {
    has_extension(path, &["wav", "aif", "aiff", "aifc"])
}

/// Up to `max_seconds` of a sound file from its start.
pub fn read(path: &Path, max_seconds: f64) -> Result<AudioData, AudioError> {
    read_from(path, max_seconds, 0)
}

/// Up to `max_seconds` of a sound file starting `from_frame` frames in, so a long file can be gone through a part at a
/// time.
pub fn read_from(path: &Path, max_seconds: f64, from_frame: u64) -> Result<AudioData, AudioError> {
    if !can_read_from(path) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !crate::other::is_other(path) {
            return Err(AudioError::NotSupported(format!("No decoder for .{ext} files")));
        }
        if from_frame > 0 {
            return Err(AudioError::NotSupported(format!(".{ext} files are only read from the start")));
        }
        return crate::other::read_other(path, max_seconds);
    }
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut magic = [0u8; 4];
    // Too short to say what it is: not a sound file. Any other failure (locked, a drive gone) stays a read error.
    f.read_exact(&mut magic).map_err(|e| match e.kind() {
        std::io::ErrorKind::UnexpectedEof => AudioError::Format("Not a WAV or AIFF file".into()),
        _ => AudioError::Io(e),
    })?;
    f.seek(SeekFrom::Start(0))?;
    match &magic {
        b"RIFF" | b"RF64" => read_wav(&mut f, len, max_seconds, from_frame),
        b"FORM" => read_aiff(&mut f, len, max_seconds, from_frame),
        _ => Err(AudioError::Format("Not a WAV or AIFF file".into())),
    }
}

/// Reads up to `n` bytes at the file's position: fewer at its end.
fn take(f: &mut File, n: u64) -> Result<Vec<u8>, AudioError> {
    let mut buf = Vec::new();
    f.by_ref().take(n).read_to_end(&mut buf)?;
    Ok(buf)
}

fn u16_le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u16_be(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_be(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

// ---- WAV ----------------------------------------------------------------------------------------------------------

fn read_wav(f: &mut File, len: u64, max_seconds: f64, from_frame: u64) -> Result<AudioData, AudioError> {
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut data: Option<(u64, u64)> = None;
    let (mut tempo, mut root, mut is_loop) = (None, None, None);

    let mut pos = 12u64; // RIFF, size, WAVE
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos))?;
        let head = take(f, 8)?;
        let id = head.get(..4).unwrap_or_default();
        let size = u64::from(u32_le(&head, 4).unwrap_or(0));
        let body = pos + 8;
        let mut next = body + size + (size & 1);
        match id {
            b"fmt " => {
                let b = take(f, size.min(64))?;
                format = u16_le(&b, 0).unwrap_or(0);
                channels = u16_le(&b, 2).unwrap_or(0);
                rate = u32_le(&b, 4).unwrap_or(0);
                bits = u16_le(&b, 14).unwrap_or(0);
                // WAVE_FORMAT_EXTENSIBLE: the real format is at the start of the sub-format GUID.
                if format == 0xFFFE && size >= 40 {
                    format = u16_le(&b, 24).unwrap_or(0);
                }
            }
            b"data" => {
                let length = if size == 0xFFFF_FFFF || body + size > len { len - body } else { size };
                data = Some((body, length));
                next = body + length + (length & 1);
            }
            b"acid" if size >= 24 => {
                let b = take(f, 24)?;
                // A chunk the file cuts short is left out rather than read as zeros (which would say "a loop").
                if b.len() < 24 {
                    pos = next;
                    continue;
                }
                let flags = u32_le(&b, 0).unwrap_or(0);
                let root_note = u16_le(&b, 4).unwrap_or(0);
                let t = f64::from(f32::from_le_bytes(b.get(20..24).and_then(|s| s.try_into().ok()).unwrap_or([0; 4])));
                is_loop = Some(flags & 1 == 0); // bit 0 marks a one-shot
                if flags & 2 != 0 && (1..128).contains(&root_note) {
                    root = Some(i32::from(root_note));
                }
                if t > 30.0 && t < 400.0 {
                    tempo = Some(t);
                }
            }
            b"smpl" if size >= 36 => {
                let b = take(f, 16)?;
                let unity = u32_le(&b, 12).map_or(0, |u| u as i32);
                // 60 is the usual "not set".
                if root.is_none() && (1..128).contains(&unity) && unity != 60 {
                    root = Some(unity);
                }
            }
            _ => {}
        }
        pos = next;
    }
    let Some((start_byte, length)) = data.filter(|_| channels > 0 && rate > 0) else {
        return Err(AudioError::Format("WAV file has no audio data".into()));
    };
    // Rounded up, so a 12-bit or 20-bit file is read with the bytes each sample really takes.
    let bytes = usize::from(bits).div_ceil(8);
    if bytes == 0 {
        return Err(AudioError::NotSupported("Unsupported WAV bit depth".into()));
    }
    let float = format == 3;
    if format != 1 && !float {
        return Err(AudioError::NotSupported(format!("WAV format {format} isn't PCM")));
    }
    // Checked before reading, so a kind of sample this can't decode never reads the file's audio first.
    let sample = sample_reader(bytes, float, false)?;
    let channels = usize::from(channels);
    let frame_bytes = (bytes * channels) as u64;
    let total = length / frame_bytes;
    let raw = read_frames(f, len, start_byte, total, from_frame, max_seconds * f64::from(rate), frame_bytes)?;
    Ok(AudioData {
        sample_rate: rate,
        channels: decode(&raw, channels, bytes, sample),
        file_seconds: total as f64 / f64::from(rate),
        embedded_tempo: tempo,
        embedded_root_note: root,
        embedded_is_loop: is_loop,
    })
}

// ---- AIFF ---------------------------------------------------------------------------------------------------------

fn read_aiff(f: &mut File, len: u64, max_seconds: f64, from_frame: u64) -> Result<AudioData, AudioError> {
    let head = take(f, 12)?;
    let aifc = head.get(8..12) == Some(b"AIFC".as_slice());
    let (mut channels, mut bits, mut total, mut rate) = (0i16, 0i16, 0u64, 0f64);
    let (mut float, mut little) = (false, false);
    let mut start: Option<u64> = None;
    let mut root = None;

    let mut pos = 12u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos))?;
        let hdr = take(f, 8)?;
        let id = hdr.get(..4).unwrap_or_default();
        let size = u64::from(u32_be(&hdr, 4).unwrap_or(0));
        let body = pos + 8;
        match id {
            b"COMM" => {
                let c = take(f, size.min(64))?;
                if c.len() < 18 {
                    return Err(AudioError::Format("AIFF file's COMM chunk is cut short".into()));
                }
                channels = u16_be(&c, 0).unwrap_or(0) as i16;
                total = u64::from(u32_be(&c, 2).unwrap_or(0));
                bits = u16_be(&c, 6).unwrap_or(0) as i16;
                rate = c.get(8..18).map_or(0.0, extended80);
                if aifc && c.len() >= 22 {
                    let comp = c.get(18..22).unwrap_or_default();
                    float = matches!(comp, b"fl32" | b"FL32" | b"fl64" | b"FL64");
                    little = comp == b"sowt";
                    if !float && !matches!(comp, b"NONE" | b"none" | b"sowt" | b"twos") {
                        let name = String::from_utf8_lossy(comp);
                        return Err(AudioError::NotSupported(format!("Compressed AIFF ({name})")));
                    }
                }
            }
            b"SSND" => {
                let off = take(f, 8)?;
                if off.len() == 8 {
                    start = Some(body + 8 + u64::from(u32_be(&off, 0).unwrap_or(0)));
                }
            }
            b"INST" if size >= 1 => {
                let b = take(f, 1)?;
                if let Some(&n) = b.first()
                    && (1..128).contains(&n)
                    && n != 60
                {
                    root = Some(i32::from(n));
                }
            }
            _ => {}
        }
        pos = (body + size + (size & 1)).min(len);
    }
    let Some(start_byte) = start.filter(|_| channels > 0 && rate > 0.0) else {
        return Err(AudioError::Format("AIFF file has no audio data".into()));
    };
    let bytes = usize::try_from(bits.max(0)).unwrap_or(0).div_ceil(8);
    if bytes == 0 {
        return Err(AudioError::NotSupported("Unsupported AIFF bit depth".into()));
    }
    let sample = sample_reader(bytes, float, !little)?;
    let channels = usize::try_from(channels).unwrap_or(0);
    let frame_bytes = (bytes * channels) as u64;
    let raw = read_frames(f, len, start_byte, total, from_frame, max_seconds * rate, frame_bytes)?;
    Ok(AudioData {
        // C# rounds half to even (Math.Round).
        sample_rate: rate.round_ties_even().min(f64::from(u32::MAX)) as u32,
        channels: decode(&raw, channels, bytes, sample),
        file_seconds: total as f64 / rate,
        embedded_root_note: root,
        ..AudioData::default()
    })
}

/// An 80-bit IEEE extended float, as AIFF keeps its sample rate.
fn extended80(b: &[u8]) -> f64 {
    let (Some(&b0), Some(&b1), Some(mant)) =
        (b.first(), b.get(1), b.get(2..10).and_then(|m| m.try_into().ok()).map(u64::from_be_bytes))
    else {
        return 0.0;
    };
    let exp = (i32::from(b0 & 0x7F) << 8) | i32::from(b1);
    if exp == 0 && mant == 0 {
        return 0.0;
    }
    let sign = if b0 & 0x80 != 0 { -1.0 } else { 1.0 };
    mant as f64 * 2f64.powi(exp - 16383 - 63) * sign
}

// ---- samples ------------------------------------------------------------------------------------------------------

/// Reads the frames to decode: from `from_frame` of `total`, up to `wanted` (truncated, as C# casts it: below zero or
/// not a number gives none, endless gives all), never more than [`MOST_BYTES`]. A file that ends early gives the whole
/// frames that are there.
fn read_frames(
    f: &mut File,
    len: u64,
    start_byte: u64,
    total: u64,
    from_frame: u64,
    wanted: f64,
    frame_bytes: u64,
) -> Result<Vec<u8>, AudioError> {
    let first = from_frame.min(total);
    // `as` saturates: NaN and anything below zero give 0, infinity gives every frame.
    let wanted = wanted as u64;
    let frames = (total - first).min(wanted).min(MOST_BYTES / frame_bytes);
    let at = start_byte.saturating_add(first.saturating_mul(frame_bytes));
    f.seek(SeekFrom::Start(at))?;
    let want = frames * frame_bytes;
    // Room for it all at once (no more than the file holds), so a big read isn't copied as it grows.
    let mut raw = Vec::with_capacity(usize::try_from(want.min(len.saturating_sub(at))).unwrap_or(0));
    f.by_ref().take(want).read_to_end(&mut raw)?;
    raw.truncate(raw.len() - raw.len() % frame_bytes as usize);
    Ok(raw)
}

/// Splits interleaved samples into one list per channel.
fn decode(raw: &[u8], channels: usize, bytes: usize, sample: fn(&[u8]) -> f32) -> Vec<Vec<f32>> {
    let frames = raw.len() / (bytes * channels).max(1);
    // One `with_capacity` each: `vec![v; n]` clones, and a clone doesn't keep the room set aside.
    let mut out: Vec<Vec<f32>> = (0..channels).map(|_| Vec::with_capacity(frames)).collect();
    for frame in raw.chunks_exact(bytes * channels) {
        for (ch, s) in out.iter_mut().zip(frame.chunks_exact(bytes)) {
            ch.push(sample(s));
        }
    }
    out
}

/// How one sample of `bytes` bytes turns into a float from -1 to 1.
fn sample_reader(bytes: usize, float: bool, big: bool) -> Result<fn(&[u8]) -> f32, AudioError> {
    fn arr<const N: usize>(b: &[u8]) -> [u8; N] {
        b.try_into().unwrap_or([0; N])
    }
    Ok(match (float, bytes, big) {
        (true, 4, false) => |b| f32::from_le_bytes(arr(b)),
        (true, 4, true) => |b| f32::from_be_bytes(arr(b)),
        (true, 8, false) => |b| f64::from_le_bytes(arr(b)) as f32,
        (true, 8, true) => |b| f64::from_be_bytes(arr(b)) as f32,
        // WAV's 8-bit samples are unsigned, AIFF's signed.
        (false, 1, false) => |b| (f32::from(b.first().copied().unwrap_or(128)) - 128.0) / 128.0,
        (false, 1, true) => |b| f32::from(b.first().copied().unwrap_or(0) as i8) / 128.0,
        (false, 2, false) => |b| f32::from(i16::from_le_bytes(arr(b))) / 32768.0,
        (false, 2, true) => |b| f32::from(i16::from_be_bytes(arr(b))) / 32768.0,
        (false, 3, false) => |b| {
            let [x, y, z] = arr::<3>(b);
            (i32::from_le_bytes([0, x, y, z]) >> 8) as f32 / 8_388_608.0
        },
        (false, 3, true) => |b| {
            let [x, y, z] = arr::<3>(b);
            (i32::from_be_bytes([x, y, z, 0]) >> 8) as f32 / 8_388_608.0
        },
        (false, 4, false) => |b| i32::from_le_bytes(arr(b)) as f32 / 2_147_483_648.0,
        (false, 4, true) => |b| i32::from_be_bytes(arr(b)) as f32 / 2_147_483_648.0,
        _ => return Err(AudioError::NotSupported(format!("{}-bit audio", bytes * 8))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_floats() {
        // 44100 as AIFF writes it.
        assert_eq!(extended80(&[0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]), 44100.0);
        assert_eq!(extended80(&[0; 10]), 0.0);
        assert_eq!(extended80(&[0x40]), 0.0);
    }

    #[test]
    fn samples() {
        let s = sample_reader(3, false, false).unwrap();
        assert_eq!(s(&[0, 0, 0x80]), -1.0);
        assert_eq!(s(&[0xFF, 0xFF, 0x7F]), 8_388_607.0 / 8_388_608.0);
        let s = sample_reader(1, false, false).unwrap();
        assert_eq!(s(&[0]), -1.0);
        let s = sample_reader(1, false, true).unwrap();
        assert_eq!(s(&[0x80]), -1.0);
        assert!(sample_reader(8, false, false).is_err());
        assert!(sample_reader(2, true, false).is_err());
    }
}
