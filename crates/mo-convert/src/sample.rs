//! What a converted project records about a sample file: its size and MD5 (how Bitwig recognises the file), and its
//! rate, channels and length. WAV (PCM, float, A-law and µ-law, RF64 too), AIFF and FLAC say those in their header.
//! Other files (MP3, OGG, M4A, compressed WAVs such as FL Studio's own sounds, which keep Ogg Vorbis inside) are
//! asked of mo-audio's decoders, which read only the file's start when the file says how long it is. One that can't
//! be timed for sure is left for Bitwig to read.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use md5::{Digest, Md5};

use crate::plan::SampleFile;

/// How much of the file's start is kept to read its header: a WAV's or AIFF's format chunks come before its audio,
/// after at most some metadata chunks.
const HEADER: usize = 1 << 20;

/// How much of a file is decoded to ask its decoder how long the file says it is ([`decoded`]).
const PEEK_SECONDS: f64 = 0.25;

/// What's known of a sample, or `None` when the file can't be read. The header is read first; only when the sample's
/// length can be told is the rest read too, once, to hash it (rule 12). When it can be read but its length can't be
/// told, the length is 0 and the MD5 is left at zeros (Bitwig then reads the file itself), and the rate and channels
/// say 44100 and 2, as the C# app's do.
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
    let chunks = wav_chunks(&head, size, &mut beyond);
    let ogg_in_wav = chunks.is_some_and(|c| mo_audio::OGG_IN_WAV.contains(&c.tag));
    // A file whose header should have said its length and didn't (a WAV of plain samples that was never finished, an
    // AIFF) isn't handed to a decoder: there is nothing more for one to find in it.
    let plain = match &chunks {
        Some(c) => c.is_plain(),
        None => head.starts_with(b"RIFF") || head.starts_with(b"RF64") || head.starts_with(b"FORM"),
    };
    let found = chunks
        .and_then(WavChunks::timed)
        .or_else(|| aiff(&head))
        .or_else(|| flac(&head))
        .filter(|&(_, _, seconds)| seconds > 0.0)
        .or_else(|| if plain { None } else { decoded(path, &head) });
    // A length of 0 is no use to Bitwig (it reads the file itself), so neither is the MD5: not worked out.
    let Some((rate, channels, seconds)) = found else {
        return Some(SampleFile { bytes: size, md5: [0; 16], rate: 44100, channels: 2, seconds: 0.0, ogg_in_wav });
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
    Some(SampleFile { bytes, md5: md5.finalize().into(), rate, channels, seconds, ogg_in_wav })
}

/// The rate, channels and seconds of a file whose header doesn't say them plainly (MP3, OGG, M4A, a WAV that keeps
/// Ogg Vorbis inside), from mo-audio's decoder. Where the file itself says how long it is, only its start is decoded;
/// one that doesn't say is decoded to its end and counted. None when the decoder can't read the file, and for the
/// files whose length a decoder can only estimate from their first frames and their size (an MP3 with no length
/// written in it, a bare AAC stream): a wrong length would cut the sample short in Bitwig, so those are left for
/// Bitwig to read. `head` is the file's start.
fn decoded(path: &Path, head: &[u8]) -> Option<(u32, u16, f64)> {
    if !mo_audio::can_decode(path) {
        return None;
    }
    let is = |ext: &str| path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(ext));
    if is("aac") || (is("mp3") && !mp3_says_its_length(head)) {
        return None;
    }
    let start = mo_audio::read(path, PEEK_SECONDS).ok()?;
    let channels = u16::try_from(start.channels.len()).ok().filter(|c| *c > 0)?;
    if start.sample_rate == 0 {
        return None;
    }
    // The file's own word counts when it says more than was decoded, or when the file ended before the peek did.
    let asked = (PEEK_SECONDS * f64::from(start.sample_rate)) as usize;
    let seconds = if start.file_seconds > start.seconds() || start.len() < asked {
        start.file_seconds
    } else {
        let mut frames = 0u64;
        mo_audio::read_parts(path, 1.0, &mut |_, part| {
            frames += part.first().map_or(0, Vec::len) as u64;
            true
        })
        .ok()?;
        frames as f64 / f64::from(start.sample_rate)
    };
    (seconds > 0.0).then_some((start.sample_rate, channels, seconds))
}

/// Whether an MP3 has its length written in it: a "Xing" or "Info" header in its first frame (where it sits depends
/// on the frame's MPEG version and channels), or a "VBRI" one. `head` is the file's start; an ID3v2 tag comes first.
fn mp3_says_its_length(head: &[u8]) -> bool {
    let mut at = 0usize;
    if head.starts_with(b"ID3") {
        let Some(size) = head.get(6..10) else { return false };
        // Four bytes of seven bits each, then the tag's own ten-byte header.
        at = 10 + size.iter().fold(0usize, |n, b| n << 7 | usize::from(b & 0x7f));
    }
    // The first frame: eleven set bits. A little room for padding after the tag.
    let Some(frame) = (at..at.saturating_add(4096))
        .find(|&i| matches!((head.get(i), head.get(i + 1)), (Some(0xff), Some(b)) if b & 0xe0 == 0xe0))
    else {
        return false;
    };
    let (Some(version), Some(mode)) = (head.get(frame + 1).map(|b| b >> 3 & 3), head.get(frame + 3).map(|b| b >> 6))
    else {
        return false;
    };
    let mono = mode == 3;
    // MPEG 1 (version bits 3) has more side information than MPEG 2 and 2.5.
    let side = match (version == 3, mono) {
        (true, false) => 32,
        (true, true) | (false, false) => 17,
        (false, true) => 9,
    };
    let tag = |offset: usize| head.get(frame + 4 + offset..frame + 8 + offset);
    matches!(tag(side), Some(b"Xing" | b"Info")) || tag(32) == Some(b"VBRI")
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

/// A WAV's (or RF64's) format, from its `fmt ` chunk, and the size of its `data` chunk, which is
/// never taken as running past the file's end (`size`): a recorder that stopped before finishing the header leaves it
/// at its largest.
/// Its chunks are walked from the head (`head`, the file's start) and past it with `beyond` (bytes at a place in the
/// file), so a data chunk after a large metadata one is still found. Chunks are padded to an even size, but some
/// writers leave an odd one (often LIST or bext) unpadded: where the padded place doesn't start a chunk and the
/// unpadded one does, the walk goes on from there.
fn wav_chunks(head: &[u8], size: u64, beyond: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>) -> Option<WavChunks> {
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
    Some(WavChunks { tag, channels, rate, block, data: data.unwrap_or(0) })
}

/// What a WAV's `fmt ` and `data` chunks say ([`wav_chunks`]).
#[derive(Clone, Copy)]
struct WavChunks {
    /// The format: 1 PCM, 3 float, 6 A-law, 7 µ-law, others compressed.
    tag: u16,
    channels: u16,
    rate: u32,
    /// Bytes in a block: one frame, in the formats that are timed here.
    block: u16,
    /// Bytes of audio.
    data: u64,
}

impl WavChunks {
    /// Plain samples, a frame to a block: PCM, float, A-law, µ-law.
    fn is_plain(self) -> bool {
        matches!(self.tag, 1 | 3 | 6 | 7)
    }

    /// The rate, channels and seconds, for the formats whose blocks hold one frame each.
    fn timed(self) -> Option<(u32, u16, f64)> {
        if !self.is_plain() {
            return None;
        }
        let bytes_per_second = self.rate as f64 * self.block as f64;
        (bytes_per_second > 0.0 && self.channels > 0)
            .then(|| (self.rate, self.channels, self.data as f64 / bytes_per_second))
    }
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

    /// A WAV's rate, channels and seconds, as [`read_sample`] takes them from its header.
    fn wav(head: &[u8], size: u64, beyond: &mut dyn FnMut(u64, usize) -> Option<Vec<u8>>) -> Option<(u32, u16, f64)> {
        wav_chunks(head, size, beyond).and_then(WavChunks::timed)
    }

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
