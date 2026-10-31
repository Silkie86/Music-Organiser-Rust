//! MP3, FLAC, M4A (AAC), OGG (Vorbis) and AAC, decoded with symphonia on every system. The C# app needed Windows'
//! decoders for these, so a Mac or Linux left them out.

use std::fs::File;
use std::io::ErrorKind;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
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
    catch_unwind(AssertUnwindSafe(|| decode(file, path, max_seconds)))
        .unwrap_or_else(|_| Err(AudioError::Format("The decoder couldn't read this file (it may be damaged)".into())))
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
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(format_error)?;
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
