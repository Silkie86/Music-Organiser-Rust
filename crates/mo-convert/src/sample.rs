//! What a converted project records about a sample file: its size and MD5 (how Bitwig recognises the file), and its
//! rate, channels and length when the app can tell from the file's header: WAV (PCM, float, A-law and µ-law, RF64
//! too), AIFF and FLAC. Other files (MP3, OGG, M4A, compressed WAVs) are left for Bitwig to read: Bitwig asks where
//! they are when the project opens (see `DIFFERENCES.md`).

use std::io::Read;
use std::path::Path;

use md5::{Digest, Md5};

use crate::plan::SampleFile;

/// How much of the file's start is kept to read its header: a WAV's or AIFF's format chunks come before its audio,
/// after at most some metadata chunks.
const HEADER: usize = 1 << 20;

/// What's known of a sample, or `None` when the file can't be read. The header is read first; only when it tells the
/// sample's length is the rest read too, once, to hash it (rule 12). When it can be read but its length can't be told,
/// the length is 0 and the MD5 is left at zeros (Bitwig then reads the file itself), and the rate and channels say
/// 44100 and 2, as the C# app's do.
pub fn read_sample(path: &Path) -> Option<SampleFile> {
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let mut head = vec![0u8; HEADER.min(size as usize)];
    read_all(&mut file, &mut head)?;
    let Some((rate, channels, seconds)) = wav(&head, size).or_else(|| aiff(&head)).or_else(|| flac(&head)) else {
        return Some(SampleFile { bytes: size, md5: [0; 16], rate: 44100, channels: 2, seconds: 0.0 });
    };
    let mut md5 = Md5::new();
    md5.update(&head);
    let mut bytes = head.len() as u64;
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        md5.update(buffer.get(..n)?);
        bytes += n as u64;
    }
    Some(SampleFile { bytes, md5: md5.finalize().into(), rate, channels, seconds })
}

/// Fills `buffer` from the file's start (the file was at least that long when asked).
fn read_all(file: &mut std::fs::File, buffer: &mut [u8]) -> Option<()> {
    let mut at = 0;
    while at < buffer.len() {
        match file.read(buffer.get_mut(at..)?) {
            Ok(0) => return None,
            Ok(n) => at += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(())
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

fn le64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// A WAV's (or RF64's) rate, channels and seconds, from its `fmt ` chunk and the size of its `data` chunk, which is
/// never taken as running past the file's end (`size`): a recorder that stopped before finishing the header leaves it
/// at its largest. Only formats whose blocks hold one frame each are timed (PCM, float, A-law, µ-law); compressed ones
/// are left for the DAW, as the C# app leaves them.
fn wav(b: &[u8], size: u64) -> Option<(u32, u16, f64)> {
    let rf64 = match b.get(..4)? {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return None,
    };
    if b.get(8..12)? != b"WAVE" {
        return None;
    }
    let (mut at, mut format, mut data, mut long_data) = (12usize, None, None, None);
    while at + 8 <= b.len() && (format.is_none() || data.is_none()) {
        let id = b.get(at..at + 4)?;
        let chunk = le32(b, at + 4)? as usize;
        match id {
            b"ds64" if rf64 => long_data = le64(b, at + 16),
            b"fmt " => {
                let mut tag = le16(b, at + 8)?;
                if tag == 0xfffe {
                    // WAVE_FORMAT_EXTENSIBLE: the real format is the start of the sub-format's GUID.
                    tag = le16(b, at + 32)?;
                }
                format = Some((tag, le16(b, at + 10)?, le32(b, at + 12)?, le16(b, at + 20)?));
            }
            b"data" => {
                let written = if rf64 && chunk == 0xffff_ffff { long_data.unwrap_or(u64::MAX) } else { chunk as u64 };
                data = Some(written.min(size.saturating_sub(at as u64 + 8)));
            }
            _ => {}
        }
        // Chunks are padded to an even size.
        at = at.checked_add(8)?.checked_add(chunk)?.checked_add(chunk & 1)?;
    }
    let (tag, channels, rate, block) = format?;
    if !matches!(tag, 1 | 3 | 6 | 7) {
        return None;
    }
    let bytes_per_second = rate as f64 * block as f64;
    (bytes_per_second > 0.0 && channels > 0).then(|| (rate, channels, data.unwrap_or(0) as f64 / bytes_per_second))
}

/// A FLAC's rate, channels and seconds, from its STREAMINFO block, which always comes first.
fn flac(b: &[u8]) -> Option<(u32, u16, f64)> {
    if b.get(..4)? != b"fLaC" || b.get(4)? & 0x7f != 0 {
        return None;
    }
    // From byte 18: 20 bits of rate, 3 of channels less one, 5 of bits per sample less one, 36 of frames.
    let bits = u64::from_be_bytes(b.get(18..26)?.try_into().ok()?);
    let rate = (bits >> 44) as u32;
    let channels = ((bits >> 41) & 7) as u16 + 1;
    let frames = bits & 0xf_ffff_ffff;
    (rate > 0 && frames > 0).then(|| (rate, channels, frames as f64 / rate as f64))
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
        assert_eq!(wav(&wav_bytes(48000, 2, 24000), u64::MAX), Some((48000, 2, 0.5)));
        assert_eq!(wav(b"RIFF\0\0\0\0WAVE", 12), None, "no format chunk");
    }

    #[test]
    fn a_wav_is_never_longer_than_its_file_or_timed_when_compressed() {
        let mut b = wav_bytes(44100, 2, 44100);
        let size = b.len() as u64;
        // A recorder that never finished the header.
        b.get_mut(40..44).unwrap().copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(wav(&b, size), Some((44100, 2, 1.0)));
        // Cut short: the header says 1 second, the file holds half.
        let half = wav_bytes(44100, 2, 44100);
        assert_eq!(wav(&half, half.len() as u64 - 88200), Some((44100, 2, 0.5)));
        // IMA ADPCM (0x11): a block holds many frames, so it's left for the DAW.
        b.get_mut(20..22).unwrap().copy_from_slice(&0x11u16.to_le_bytes());
        assert_eq!(wav(&b, size), None);
    }

    #[test]
    fn an_rf64s_length_comes_from_its_ds64_chunk() {
        let pcm = wav_bytes(48000, 1, 48000);
        let mut b = b"RF64\xff\xff\xff\xffWAVEds64".to_vec();
        b.extend_from_slice(&28u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&96000u64.to_le_bytes());
        b.extend_from_slice(&[0; 12]);
        // The PCM file's fmt chunk, then a data chunk whose size is in ds64.
        b.extend_from_slice(pcm.get(12..36).unwrap());
        b.extend_from_slice(b"data\xff\xff\xff\xff");
        b.resize(b.len() + 96000, 0);
        assert_eq!(wav(&b, b.len() as u64), Some((48000, 1, 1.0)));
    }

    #[test]
    fn a_flacs_length_comes_from_its_streaminfo() {
        let mut b = b"fLaC\0\0\0\x22".to_vec();
        b.resize(18, 0);
        // 44100 Hz, 2 channels, 16 bits, 88200 frames.
        let bits = (44100u64 << 44) | (1 << 41) | (15 << 36) | 88200;
        b.extend_from_slice(&bits.to_be_bytes());
        b.resize(42, 0);
        assert_eq!(flac(&b), Some((44100, 2, 2.0)));
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
