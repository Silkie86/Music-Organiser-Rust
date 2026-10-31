//! MP3, FLAC, M4A (AAC), OGG (Vorbis) and AAC, decoded with symphonia on every system. The C# app needed Windows'
//! decoders for these, so a Mac or Linux left them out.

use std::fs::File;
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
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| OTHER_EXTENSIONS.iter().any(|o| o.eq_ignore_ascii_case(e)))
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
        Error::IoError(e) => AudioError::Io(e),
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
            // A damaged or cut-short stretch: keep what came before it.
            Err(Error::IoError(_) | Error::DecodeError(_)) if !channels.is_empty() => break,
            Err(e) => return Err(format_error(e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            // One damaged frame is skipped, as players do.
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format_error(e)),
        };
        if rate == 0 {
            rate = buf.spec().rate();
            let want = max_seconds * f64::from(rate);
            wanted = if want.is_nan() { 0 } else { want as usize };
        }
        buf.copy_to_vecs_planar(&mut scratch);
        if channels.is_empty() {
            channels = vec![Vec::new(); scratch.len()];
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
