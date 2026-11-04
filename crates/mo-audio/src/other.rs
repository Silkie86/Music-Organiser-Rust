//! MP3, FLAC, M4A (AAC), OGG (Vorbis) and AAC, decoded with symphonia on every system. The C# app needed Windows'
//! decoders for these, so a Mac or Linux left them out.

use std::fs::File;
use std::io::{Cursor, ErrorKind, Read, Seek, SeekFrom};
use std::panic::AssertUnwindSafe;
use std::path::Path;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

use crate::{AudioData, AudioError};

/// The kinds symphonia decodes here, by extension.
pub const OTHER_EXTENSIONS: [&str; 5] = ["mp3", "flac", "m4a", "ogg", "aac"];

/// Whether this file is one of [`OTHER_EXTENSIONS`] (ignoring case).
pub fn is_other(path: &Path) -> bool {
    crate::read::has_extension(path, &OTHER_EXTENSIONS)
}

/// Up to `max_seconds` from the start of an MP3, FLAC, M4A, OGG or AAC file. A damaged stretch part way in ends the
/// read there with what came before it; a decoder that falls over on a damaged file gives an error, never a crash.
pub fn read_other(path: &Path, max_seconds: f64) -> Result<AudioData, AudioError> {
    let file = File::open(path)?;
    mo_catch::catch(AssertUnwindSafe(|| decode(file, path, max_seconds)))
        .unwrap_or_else(|_| Err(AudioError::Format("The decoder couldn't read this file (it may be damaged)".into())))
}

/// The biggest file whose audio is read into memory to be tried past a damaged ID3v2 tag.
const MOST_RETRIED: u64 = 256 * 1024 * 1024;

/// The bytes after a file's leading ID3v2 tag, or after its 10-byte header when the tag says it's longer than the
/// file (cut short). None when the file has no such tag, or is too big to read whole.
fn after_id3v2(file: &File) -> Option<Vec<u8>> {
    let len = file.metadata().ok()?.len();
    if len > MOST_RETRIED {
        return None;
    }
    let mut file = file;
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut all = Vec::new();
    file.read_to_end(&mut all).ok()?;
    let header: [u8; 10] = *all.first_chunk::<10>().filter(|h| h.starts_with(b"ID3"))?;
    // The size is synchsafe (7 bits a byte); a footer (flag 0x10) adds 10 more bytes.
    let size = header[6..10].iter().fold(0usize, |n, &b| n << 7 | usize::from(b & 0x7F));
    let footer = if header[5] & 0x10 != 0 { 10 } else { 0 };
    let end = 10 + size + footer;
    let from = if end < all.len() { end } else { 10 };
    Some(all.split_off(from))
}

fn decode(file: File, path: &Path, max_seconds: f64) -> Result<AudioData, AudioError> {
    let format_error = |e: Error| match e {
        // symphonia gives an I/O error for a file that ends too soon or a bitstream that runs out: that's a damaged
        // file, not a drive or lock problem (as the WAV reader says it).
        Error::IoError(e) if e.kind() != ErrorKind::UnexpectedEof && e.kind() != ErrorKind::Other => AudioError::Io(e),
        Error::IoError(_) => AudioError::Format("The file is cut short or damaged".into()),
        Error::Unsupported(what) => AudioError::NotSupported(format!("Not supported: {what}")),
        other => AudioError::Format(other.to_string()),
    };
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probe = |source: Box<dyn MediaSource>| {
        let mss = MediaSourceStream::new(source, Default::default());
        symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
    };
    let mut format = match probe(Box::new(file.try_clone()?)) {
        Ok(format) => format,
        // A leading ID3v2 tag that's damaged (a version or header symphonia refuses, or a size past the end of the
        // file) stops symphonia's tag reader. Before it had one, the probe looked past the tag for the first frame:
        // the audio after the tag is tried that way instead.
        Err(e) => match after_id3v2(&file) {
            Some(rest) => probe(Box::new(Cursor::new(rest))).map_err(|_| format_error(e))?,
            None => return Err(format_error(e)),
        },
    };
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| AudioError::Format("The file has no audio in it".into()))?;
    let track_id = track.id;
    let known_frames = track.num_frames;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| AudioError::NotSupported("A kind of audio this app doesn't decode".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(format_error)?;

    let mut rate = 0u32;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<Vec<f32>> = Vec::new();
    let mut wanted = usize::MAX;
    let mut whole = true;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // A damaged or cut-short stretch: keep what came before it. A real read failure (a drive gone) still
            // says so, rather than passing a part off as the whole file.
            Err(Error::DecodeError(_)) if !channels.is_empty() => break,
            Err(Error::IoError(e)) if !channels.is_empty() && e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format_error(e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            // One damaged frame is skipped, as players do. A frame whose bits run out comes as an I/O error: the
            // packet is already in memory, so that's damage too.
            Err(Error::DecodeError(_) | Error::IoError(_)) => continue,
            Err(e) => return Err(format_error(e)),
        };
        let spec = buf.spec();
        if rate == 0 {
            rate = spec.rate();
            let count = spec.channels().count().max(1);
            // `as` saturates: NaN and anything below zero give 0, infinity gives every frame. Never more than one
            // read may hold, as the WAV reader.
            let most = usize::try_from(crate::read::MOST_BYTES).unwrap_or(usize::MAX) / (4 * count);
            wanted = ((max_seconds * f64::from(rate)) as usize).min(most);
            let room = known_frames.map_or(0, |k| usize::try_from(k).unwrap_or(usize::MAX)).min(wanted);
            channels = (0..count).map(|_| Vec::with_capacity(room)).collect();
        } else if spec.rate() != rate {
            // The sound changes part way (a chained stream): keep the part that matches.
            break;
        }
        buf.copy_to_vecs_planar(&mut scratch);
        if scratch.len() != channels.len() {
            break;
        }
        let have = channels.first().map_or(0, Vec::len);
        let room = wanted.saturating_sub(have);
        for (ch, s) in channels.iter_mut().zip(&scratch) {
            ch.extend_from_slice(s.get(..room.min(s.len())).unwrap_or_default());
        }
        if channels.first().map_or(0, Vec::len) >= wanted {
            whole = false;
            break;
        }
    }
    if rate == 0 || channels.is_empty() {
        return Err(AudioError::Format("The file has no audio in it".into()));
    }
    let decoded = channels.first().map_or(0, Vec::len) as u64;
    // The whole length: what the file says, else what was decoded when it was all decoded.
    let total = known_frames.filter(|_| !whole).unwrap_or(decoded).max(decoded);
    Ok(AudioData { sample_rate: rate, file_seconds: total as f64 / f64::from(rate), channels, ..AudioData::default() })
}
