//! What a converted project records about a sample file: its size and MD5 (how Bitwig recognises the file), and its
//! rate, channels and length when the app can tell from the file's header: WAV (PCM, float, A-law and µ-law, RF64
//! too), AIFF and FLAC. Other files (MP3, OGG, M4A, compressed WAVs) are left for Bitwig to read: Bitwig asks where
//! they are when the project opens (see `DIFFERENCES.md`).

use std::io::{Read, Seek, SeekFrom};
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
    // A WAV's chunks past the head are looked at one header at a time.
    let mut beyond = |at: u64, len: usize| {
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut b = vec![0u8; len];
        read_all(&mut file, &mut b).map(|()| b)
    };
    let found = wav(&head, size, &mut beyond).or_else(|| aiff(&head)).or_else(|| flac(&head));
    // A length of 0 is no use to Bitwig (it reads the file itself), so neither is the MD5: not worked out.
    let Some((rate, channels, seconds)) = found.filter(|&(_, _, seconds)| seconds > 0.0) else {
        return Some(SampleFile { bytes: size, md5: [0; 16], rate: 44100, channels: 2, seconds: 0.0 });
    };
    file.seek(SeekFrom::Start(head.len() as u64)).ok()?;
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
/// Its chunks are walked from the head (`head`, the file's start) and past it with `beyond` (bytes at a place in the
/// file), so a data chunk after a large metadata one is still found. Chunks are padded to an even size, but some
/// writers leave an odd one (often LIST or bext) unpadded: where the padded place doesn't start a chunk and the
/// unpadded one does, the walk goes on from there.
fn wav(head: &[u8], size: u64, beyond: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>) -> Option<(u32, u16, f64)> {
    let mut get = |at: u64, len: usize| -> Option<Vec<u8>> {
        let start = usize::try_from(at).ok()?;
        match head.get(start..start.checked_add(len)?) {
            Some(b) => Some(b.to_vec()),
            None if at.checked_add(len as u64)? <= size => beyond(at, len),
            None => None,
        }
    };
    let rf64 = match head.get(..4)? {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return None,
    };
    if head.get(8..12)? != b"WAVE" {
        return None;
    }
    // What starts at a place: a chunk this needs, another plausible chunk (a printable id and a size that fits in
    // the file), or neither.
    let looks = |get: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>, at: u64| -> u8 {
        let Some(h) = get(at, 8) else { return 0 };
        let (Some(id), Some(chunk)) = (h.get(..4), le32(&h, 4)) else { return 0 };
        if matches!(id, b"data" | b"fmt ") {
            2
        } else if id.iter().all(|c| (0x20..=0x7e).contains(c))
            && at.saturating_add(8).saturating_add(u64::from(chunk)) <= size
        {
            1
        } else {
            0
        }
    };
    let (mut at, mut format, mut data, mut long_data) = (12u64, None, None, None);
    for _ in 0..MOST_CHUNKS {
        if format.is_some() && data.is_some() {
            break;
        }
        let Some(h) = get(at, 8) else { break };
        let chunk = u64::from(le32(&h, 4)?);
        match h.get(..4)? {
            b"ds64" if rf64 => long_data = le64(&get(at, 8 + chunk.min(24) as usize)?, 16),
            b"fmt " => {
                let b = get(at, 8 + chunk.min(32) as usize)?;
                let mut tag = le16(&b, 8)?;
                if tag == 0xfffe {
                    // WAVE_FORMAT_EXTENSIBLE: the real format is the start of the sub-format's GUID.
                    tag = le16(&b, 32)?;
                }
                format = Some((tag, le16(&b, 10)?, le32(&b, 12)?, le16(&b, 20)?));
            }
            b"data" => {
                let written = if rf64 && chunk == 0xffff_ffff { long_data.unwrap_or(u64::MAX) } else { chunk };
                data = Some(written.min(size.saturating_sub(at.saturating_add(8))));
            }
            _ => {}
        }
        let next = at.checked_add(8)?.checked_add(chunk)?;
        let padded = next.checked_add(chunk & 1)?;
        at = if padded != next && looks(&mut get, next) > looks(&mut get, padded) { next } else { padded };
    }
    let (tag, channels, rate, block) = format?;
    if !matches!(tag, 1 | 3 | 6 | 7) {
        return None;
    }
    let bytes_per_second = rate as f64 * block as f64;
    (bytes_per_second > 0.0 && channels > 0).then(|| (rate, channels, data.unwrap_or(0) as f64 / bytes_per_second))
}

/// How many of a WAV's chunks are looked at before giving up: a real file has a handful.
const MOST_CHUNKS: usize = 4096;

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

    /// A WAV read from its bytes alone.
    fn wav_of(b: &[u8], size: u64) -> Option<(u32, u16, f64)> {
        wav(b, size, &mut |at, len| {
            let at = usize::try_from(at).ok()?;
            b.get(at..at + len).map(<[u8]>::to_vec)
        })
    }

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
        assert_eq!(wav_of(&wav_bytes(48000, 2, 24000), u64::MAX), Some((48000, 2, 0.5)));
        assert_eq!(wav_of(b"RIFF\0\0\0\0WAVE", 12), None, "no format chunk");
    }

    #[test]
    fn a_wav_is_never_longer_than_its_file_or_timed_when_compressed() {
        let mut b = wav_bytes(44100, 2, 44100);
        let size = b.len() as u64;
        // A recorder that never finished the header.
        b.get_mut(40..44).unwrap().copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(wav_of(&b, size), Some((44100, 2, 1.0)));
        // Cut short: the header says 1 second, the file holds half.
        let half = wav_bytes(44100, 2, 44100);
        assert_eq!(wav_of(&half, half.len() as u64 - 88200), Some((44100, 2, 0.5)));
        // IMA ADPCM (0x11): a block holds many frames, so it's left for the DAW.
        b.get_mut(20..22).unwrap().copy_from_slice(&0x11u16.to_le_bytes());
        assert_eq!(wav_of(&b, size), None);
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
        assert_eq!(wav_of(&b, b.len() as u64), Some((48000, 1, 1.0)));
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

    #[test]
    fn a_wav_with_an_unpadded_odd_chunk_or_a_large_one_first_still_has_its_length() {
        let pcm = wav_bytes(44100, 2, 44100);
        let (fmt, data) = (pcm.get(12..36).unwrap(), pcm.get(36..).unwrap());
        // fmt, a LIST chunk of 0x4F bytes with no pad byte, then data.
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        b.extend_from_slice(fmt);
        b.extend_from_slice(b"LIST\x4f\0\0\0");
        b.resize(b.len() + 0x4f, b'x');
        b.extend_from_slice(data);
        assert_eq!(wav_of(&b, b.len() as u64), Some((44100, 2, 1.0)));
        // The same with a data size whose low byte reads as a letter ("ataP" where the pad byte would be).
        let mut odd = b.clone();
        let size_at = odd.len() - data.len() + 4;
        odd.get_mut(size_at..size_at + 4).unwrap().copy_from_slice(&0x0001_5F50u32.to_le_bytes());
        odd.truncate(size_at + 4 + 0x0001_5F50);
        assert_eq!(wav_of(&odd, odd.len() as u64).map(|w| w.2), Some(0x0001_5F50 as f64 / 176_400.0));
        // Padded as it should be, it's read as before.
        let at = b.len() - data.len();
        b.insert(at, 0);
        assert_eq!(wav_of(&b, b.len() as u64), Some((44100, 2, 1.0)));
        // A 2 MB chunk before the audio: its data chunk is past the head that's kept.
        let mut big = b"RIFF\0\0\0\0WAVE".to_vec();
        big.extend_from_slice(fmt);
        big.extend_from_slice(b"JUNK");
        big.extend_from_slice(&(2u32 << 20).to_le_bytes());
        big.resize(big.len() + (2 << 20), 0);
        big.extend_from_slice(data);
        let head = big.get(..HEADER).unwrap();
        let read = wav(head, big.len() as u64, &mut |at, len| {
            let at = usize::try_from(at).ok()?;
            big.get(at..at + len).map(<[u8]>::to_vec)
        });
        assert_eq!(read, Some((44100, 2, 1.0)));
        assert_eq!(wav(head, big.len() as u64, &mut |_, _| None).map(|w| w.2), Some(0.0), "the head alone");
    }
}
