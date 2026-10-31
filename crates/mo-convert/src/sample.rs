//! What a converted project records about a sample file: its size and MD5 (how Bitwig recognises the file), and its
//! rate, channels and length when the app can tell (WAV and AIFF, read from their headers; other formats are left for
//! the DAW to read, as the C# app leaves the ones it can't decode).

use std::io::Read;
use std::path::Path;

use md5::{Digest, Md5};

use crate::plan::SampleFile;

/// How much of the file's start is kept to read its header: a WAV's or AIFF's format chunks come before its audio,
/// after at most some metadata chunks.
const HEADER: usize = 1 << 20;

/// What's known of a sample, or `None` when the file can't be read. The file is read once, start to end (rule 12).
/// When it can be read but its length can't be told, the length is 0 (the rate and channels then say 44100 and 2, as
/// the C# app's do).
pub fn read_sample(path: &Path) -> Option<SampleFile> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut md5 = Md5::new();
    let mut head = Vec::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut bytes = 0u64;
    loop {
        let n = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        let got = buffer.get(..n)?;
        md5.update(got);
        if head.len() < HEADER {
            head.extend_from_slice(got.get(..n.min(HEADER - head.len()))?);
        }
        bytes += n as u64;
    }
    let (rate, channels, seconds) = wav(&head).or_else(|| aiff(&head)).unwrap_or((44100, 2, 0.0));
    Some(SampleFile { bytes, md5: md5.finalize().into(), rate, channels, seconds })
}

fn le16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// A WAV's rate, channels and seconds, from its `fmt ` chunk and the size of its `data` chunk.
fn wav(b: &[u8]) -> Option<(u32, u16, f64)> {
    if b.get(..4)? != b"RIFF" || b.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut at, mut format, mut data) = (12usize, None, None);
    while at + 8 <= b.len() && (format.is_none() || data.is_none()) {
        let id = b.get(at..at + 4)?;
        let size = le32(b, at + 4)? as usize;
        match id {
            b"fmt " => format = Some((le16(b, at + 10)?, le32(b, at + 12)?, le16(b, at + 20)?)),
            b"data" => data = Some(size),
            _ => {}
        }
        // Chunks are padded to an even size.
        at = at.checked_add(8)?.checked_add(size)?.checked_add(size & 1)?;
    }
    let (channels, rate, block) = format?;
    let bytes_per_second = rate as f64 * block as f64;
    (bytes_per_second > 0.0 && channels > 0).then(|| (rate, channels, data.unwrap_or(0) as f64 / bytes_per_second))
}

/// An AIFF's (or AIFF-C's) rate, channels and seconds, from its `COMM` chunk.
fn aiff(b: &[u8]) -> Option<(u32, u16, f64)> {
    if b.get(..4)? != b"FORM" || !matches!(b.get(8..12)?, b"AIFF" | b"AIFC") {
        return None;
    }
    let mut at = 12usize;
    while at + 8 <= b.len() {
        let size = be32(b, at + 4)? as usize;
        if b.get(at..at + 4)? == b"COMM" {
            let channels = be16(b, at + 8)?;
            let frames = be32(b, at + 10)?;
            let rate = extended(b.get(at + 16..at + 26)?)?;
            return (rate > 0.0 && channels > 0).then(|| (rate.round() as u32, channels, frames as f64 / rate));
        }
        at = at.checked_add(8)?.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

/// An 80-bit IEEE extended float (AIFF's sample rate).
fn extended(b: &[u8]) -> Option<f64> {
    let exponent = (be16(b, 0)? & 0x7fff) as i32;
    let mantissa = u64::from_be_bytes(b.get(2..10)?.try_into().ok()?);
    if exponent == 0 && mantissa == 0 {
        return Some(0.0);
    }
    Some(mantissa as f64 * 2f64.powi(exponent - 16383 - 63))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_bytes(rate: u32, channels: u16, frames: u32) -> Vec<u8> {
        let block = channels * 2;
        let data = frames * block as u32;
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * block as u32).to_le_bytes());
        b.extend_from_slice(&block.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        b.resize(b.len() + data as usize, 0);
        b
    }

    #[test]
    fn a_wavs_length_comes_from_its_header() {
        assert_eq!(wav(&wav_bytes(48000, 2, 24000)), Some((48000, 2, 0.5)));
        assert_eq!(wav(b"RIFF\0\0\0\0WAVE"), None, "no format chunk");
    }

    #[test]
    fn an_aiffs_length_comes_from_its_comm_chunk() {
        let mut b = b"FORM\0\0\0\x2eAIFFCOMM\0\0\0\x12".to_vec();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&44100u32.to_be_bytes());
        b.extend_from_slice(&16u16.to_be_bytes());
        // 44100 as an 80-bit float.
        b.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]);
        let (rate, channels, seconds) = aiff(&b).unwrap();
        assert_eq!((rate, channels), (44100, 1));
        assert!((seconds - 1.0).abs() < 1e-9);
    }
}
