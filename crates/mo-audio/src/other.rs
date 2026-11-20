//! MP3, FLAC, M4A (AAC), OGG (Vorbis) and AAC, decoded with symphonia on every system. The C# app needed Windows'
//! decoders for these, so a Mac or Linux left them out.

use std::fs::File;
use std::io::{Cursor, ErrorKind, Read, Seek, SeekFrom};
use std::panic::AssertUnwindSafe;
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration, TimeBase};

use crate::parts::{Each, Ended, Parts};
use crate::{AudioData, AudioError};

/// The kinds symphonia decodes here, by extension.
pub const OTHER_EXTENSIONS: [&str; 5] = ["mp3", "flac", "m4a", "ogg", "aac"];

/// Whether this file is one of [`OTHER_EXTENSIONS`] (ignoring case).
pub fn is_other(path: &Path) -> bool {
    crate::read::has_extension(path, &OTHER_EXTENSIONS)
}

/// Up to `max_seconds` from the start of an MP3, FLAC, M4A, OGG or AAC file. A damaged stretch part way in ends the
/// read there with what came before it (a frame damaged on its own is silence); a decoder that falls over on a damaged
/// file gives an error, never a crash.
pub fn read_other(path: &Path, max_seconds: f64) -> Result<AudioData, AudioError> {
    let file = File::open(path)?;
    mo_catch::catch(AssertUnwindSafe(|| decode(file, path, max_seconds)))
        .unwrap_or_else(|_| Err(AudioError::Format("The decoder couldn't read this file (it may be damaged)".into())))
}

/// A whole MP3, FLAC, M4A, OGG or AAC file from its start, decoded once and handed to `each` a part of up to
/// `part_seconds` at a time ([`crate::read_parts`]): never more than a part of its sound in memory. Damage is handled
/// as [`read_other`] handles it. Its whole length in seconds.
pub(crate) fn read_other_parts(path: &Path, part_seconds: f64, each: &mut Each) -> Result<f64, AudioError> {
    let file = File::open(path)?;
    mo_catch::catch(AssertUnwindSafe(|| decode_parts(probe(file, path)?, part_seconds, each)))
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

/// What a symphonia error means here.
fn format_error(e: Error) -> AudioError {
    match e {
        // symphonia gives an I/O error for a file that ends too soon or a bitstream that runs out: that's a damaged
        // file, not a drive or lock problem (as the WAV reader says it).
        Error::IoError(e) if e.kind() != ErrorKind::UnexpectedEof && e.kind() != ErrorKind::Other => AudioError::Io(e),
        Error::IoError(_) => AudioError::Format("The file is cut short or damaged".into()),
        Error::Unsupported(what) => AudioError::NotSupported(format!("Not supported: {what}")),
        other => AudioError::Format(other.to_string()),
    }
}

/// Up to `max_seconds` of an Ogg Vorbis stream held in memory, starting `from_frame` frames in: the audio of a WAV file
/// that keeps Ogg Vorbis inside it (FL Studio's own sounds). Damage is handled as [`read_other`] handles it.
pub(crate) fn read_ogg_bytes(stream: Vec<u8>, max_seconds: f64, from_frame: u64) -> Result<AudioData, AudioError> {
    mo_catch::catch(AssertUnwindSafe(|| decode_format(probe_ogg(stream)?, max_seconds, from_frame)))
        .unwrap_or_else(|_| Err(AudioError::Format("The decoder couldn't read this file (it may be damaged)".into())))
}

/// The whole of an Ogg Vorbis stream held in memory (a WAV file's: [`read_ogg_bytes`]), handed to `each` a part of up
/// to `part_seconds` at a time ([`crate::read_parts`]). Damage is handled as [`read_other`] handles it. Its whole
/// length in seconds.
pub(crate) fn read_ogg_bytes_parts(stream: Vec<u8>, part_seconds: f64, each: &mut Each) -> Result<f64, AudioError> {
    mo_catch::catch(AssertUnwindSafe(|| decode_parts(probe_ogg(stream)?, part_seconds, each)))
        .unwrap_or_else(|_| Err(AudioError::Format("The decoder couldn't read this file (it may be damaged)".into())))
}

/// An Ogg stream held in memory, ready to decode.
fn probe_ogg(stream: Vec<u8>) -> Result<Box<dyn FormatReader>, AudioError> {
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let mss = MediaSourceStream::new(Box::new(Cursor::new(stream)), Default::default());
    symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(format_error)
}

fn decode(file: File, path: &Path, max_seconds: f64) -> Result<AudioData, AudioError> {
    decode_format(probe(file, path)?, max_seconds, 0)
}

/// What kind of file it is (its extension is tried first), ready to decode.
fn probe(file: File, path: &Path) -> Result<Box<dyn FormatReader>, AudioError> {
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let look = |source: Box<dyn MediaSource>| {
        let mss = MediaSourceStream::new(source, Default::default());
        symphonia::default::get_probe().probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
    };
    match look(Box::new(file.try_clone()?)) {
        Ok(format) => Ok(format),
        // A leading ID3v2 tag that's damaged (a version or header symphonia refuses, or a size past the end of the
        // file) stops symphonia's tag reader. Before it had one, the probe looked past the tag for the first frame:
        // the audio after the tag is tried that way instead.
        Err(e) => match after_id3v2(&file) {
            Some(rest) => look(Box::new(Cursor::new(rest))).map_err(|_| format_error(e)),
            None => Err(format_error(e)),
        },
    }
}

/// A probed file's audio ([`audio_track`]).
struct AudioTrack {
    /// The id its packets carry.
    id: u32,
    /// How many frames the file says it has, when it says.
    frames: Option<u64>,
    /// What its packets' times count in, when the file says, and its sample rate as the file says it: how long a
    /// packet is in frames ([`frames_in`]).
    time_base: Option<TimeBase>,
    rate: Option<u32>,
    decoder: Box<dyn AudioDecoder>,
}

/// A probed file's audio track, and a decoder for it.
fn audio_track(format: &dyn FormatReader) -> Result<AudioTrack, AudioError> {
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| AudioError::Format("The file has no audio in it".into()))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| AudioError::NotSupported("A kind of audio this app doesn't decode".into()))?;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(format_error)?;
    Ok(AudioTrack {
        id: track.id,
        frames: track.num_frames,
        time_base: track.time_base,
        rate: params.sample_rate,
        decoder,
    })
}

/// The most frames a packet holds (FLAC's biggest block), whatever a damaged file says its length is.
const MOST_IN_A_PACKET: u64 = 65_536;

/// How many frames a packet `dur` long holds: its length counts in the track's `time_base`, at `rate` frames a second
/// (the sound's, or what the file says before any is decoded); with neither known, in frames.
fn frames_in(dur: Duration, time_base: Option<TimeBase>, rate: Option<u32>) -> u64 {
    let frames = match (time_base, rate) {
        (Some(tb), Some(rate)) if rate != 0 => {
            u128::from(dur.get()) * u128::from(tb.numer.get()) * u128::from(rate) / u128::from(tb.denom.get())
        }
        _ => u128::from(dur.get()),
    };
    u64::try_from(frames).unwrap_or(u64::MAX).min(MOST_IN_A_PACKET)
}

/// Hands `frames` of silence to `take`, `count` channels at `rate`, a packet's worth at a time: frames refused as
/// damaged ([`each_stretch`]). False once `take` says to stop.
fn silence(rate: u32, count: usize, frames: u64, take: &mut dyn FnMut(u32, &[Vec<f32>]) -> bool) -> bool {
    let mut left = frames;
    while left > 0 {
        let n = left.min(MOST_IN_A_PACKET);
        let piece = vec![vec![0.0f32; n as usize]; count];
        if !take(rate, &piece) {
            return false;
        }
        left -= n;
    }
    true
}

/// Goes through a probed file's audio (`track`) from its start, handing each stretch its decoder gives to `take`: its
/// sample rate, and its samples, a channel each, as long as each other. Until the file ends, damage ends it (what came
/// before is kept), or `take` says to stop (false). A frame that's damaged on its own is silence of its length, so what
/// comes after it stays in its place, as players keep their time, and the length comes out right. Another stream
/// joined on (a chained Ogg file: two joined into one) is gone on into with a decoder of its own, as players do, for
/// as long as its sound matches the first one's. Damage before any sound, or a real read failure (a drive gone), is an
/// error rather than a part passed off as the whole file. Both [`decode_format`] and [`decode_parts`] go through a
/// file this way, so they read it the same.
fn each_stretch(
    format: &mut dyn FormatReader,
    track: AudioTrack,
    take: &mut dyn FnMut(u32, &[Vec<f32>]) -> bool,
) -> Result<Ended, AudioError> {
    let AudioTrack { id: mut track_id, mut decoder, mut time_base, rate: mut told_rate, .. } = track;
    let mut rate = 0u32;
    let mut count = 0usize;
    let mut scratch: Vec<Vec<f32>> = Vec::new();
    // Frames refused as damaged since the last stretch: silence, handed on before the next one.
    let mut owed = 0u64;
    let ended = loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break Ended::End,
            // The first page of a stream joined on: its packets come with an id and decoder of their own. Where it
            // has no sound this app decodes, the sound ends with the stream before it.
            Err(Error::ResetRequired) => match audio_track(format) {
                Ok(next) => {
                    (track_id, decoder, time_base, told_rate) = (next.id, next.decoder, next.time_base, next.rate);
                    continue;
                }
                Err(_) if rate != 0 => break Ended::End,
                Err(e) => return Err(e),
            },
            Err(Error::DecodeError(_)) if rate != 0 => break Ended::Damage,
            Err(Error::IoError(e)) if rate != 0 && e.kind() == ErrorKind::UnexpectedEof => break Ended::Damage,
            Err(e) => return Err(format_error(e)),
        };
        if packet.track_id != track_id {
            continue;
        }
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            // A frame damaged on its own (one whose bits run out comes as an I/O error: the packet is already in
            // memory, so that's damage too), or each one after an MP3's sound changes part way (mono to stereo), which
            // its decoder refuses: silence of the length it would have given. The decoder takes an encoder's delay and
            // padding off a stream's first and last packets, and symphonia's MP3 reader counts them in a packet's
            // length; its Ogg reader leaves them out already, so a refused last Ogg packet counts that much short,
            // which moves nothing.
            Err(Error::DecodeError(_) | Error::IoError(_)) => {
                let valid =
                    packet.dur.get().saturating_sub(packet.trim_start.get()).saturating_sub(packet.trim_end.get());
                owed += frames_in(Duration::new(valid), time_base, Some(rate).filter(|&r| r != 0).or(told_rate));
                continue;
            }
            Err(e) => return Err(format_error(e)),
        };
        let spec = buf.spec();
        if rate == 0 {
            rate = spec.rate();
            count = spec.channels().count().max(1);
        } else if spec.rate() != rate {
            // The sound changes part way (a chained stream): the part that matches.
            break Ended::End;
        }
        buf.copy_to_vecs_planar(&mut scratch);
        if scratch.len() != count {
            break Ended::End;
        }
        if owed > 0 && !silence(rate, count, std::mem::take(&mut owed), take) {
            return Ok(Ended::Stopped);
        }
        if !take(rate, &scratch) {
            return Ok(Ended::Stopped);
        }
    };
    // Frames refused at the end are silence too, so the sound keeps its length. That's what came, as the read went to
    // the end: the length the file says is no surer (for an MP3 without a header saying it, it's worked out from its
    // size).
    if rate != 0 && owed > 0 && !silence(rate, count, owed, take) {
        return Ok(Ended::Stopped);
    }
    Ok(ended)
}

/// Decodes up to `max_seconds` of a probed file's audio, leaving out its first `skip` frames.
fn decode_format(mut format: Box<dyn FormatReader>, max_seconds: f64, skip: u64) -> Result<AudioData, AudioError> {
    let mut skip = usize::try_from(skip).unwrap_or(usize::MAX);
    let mut skipped = 0u64;
    let track = audio_track(format.as_ref())?;
    let known_frames = track.frames;
    let mut sample_rate = 0u32;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut wanted = usize::MAX;
    let ended = each_stretch(format.as_mut(), track, &mut |rate, stretch| {
        if channels.is_empty() {
            sample_rate = rate;
            // `as` saturates: NaN and anything below zero give 0, infinity gives every frame. Never more than one
            // read may hold, as the WAV reader.
            let most = usize::try_from(crate::read::MOST_BYTES).unwrap_or(usize::MAX) / (4 * stretch.len());
            wanted = ((max_seconds * f64::from(rate)) as usize).min(most);
            let room = known_frames.map_or(0, |k| usize::try_from(k).unwrap_or(usize::MAX)).min(wanted);
            channels = (0..stretch.len()).map(|_| Vec::with_capacity(room)).collect();
        }
        let have = channels.first().map_or(0, Vec::len);
        let room = wanted.saturating_sub(have);
        // The frames before the start asked for are decoded and left out.
        let drop = skip.min(stretch.first().map_or(0, Vec::len));
        skip -= drop;
        skipped += drop as u64;
        for (ch, s) in channels.iter_mut().zip(stretch) {
            let s = s.get(drop..).unwrap_or_default();
            ch.extend_from_slice(s.get(..room.min(s.len())).unwrap_or_default());
        }
        channels.first().map_or(0, Vec::len) < wanted
    })?;
    if sample_rate == 0 || channels.is_empty() {
        return Err(AudioError::Format("The file has no audio in it".into()));
    }
    let decoded = skipped + channels.first().map_or(0, Vec::len) as u64;
    // The whole length: what the file says, else what was decoded when it was all decoded (to its end or to damage).
    let whole = ended != Ended::Stopped;
    let total = known_frames.filter(|_| !whole).unwrap_or(decoded).max(decoded);
    Ok(AudioData { sample_rate, file_seconds: total as f64 / f64::from(sample_rate), channels, ..AudioData::default() })
}

/// The whole of a probed file's audio, handed to `each` a part of up to `part_seconds` at a time, in order, with its
/// sample rate ([`read_other_parts`]); `each` says whether to go on. The file is gone through as [`decode_format`]
/// goes through it ([`each_stretch`]). The whole length in seconds: what was decoded, or when damage or `each` ended
/// it early, what the file says if it's more, so a clip that plays the whole file still plays it at its speed.
fn decode_parts(mut format: Box<dyn FormatReader>, part_seconds: f64, each: &mut Each) -> Result<f64, AudioError> {
    let track = audio_track(format.as_ref())?;
    let known_frames = track.frames;
    let mut parts: Option<Parts> = None;
    let ended = each_stretch(format.as_mut(), track, &mut |rate, stretch| {
        parts.get_or_insert_with(|| Parts::new(rate, stretch.len(), part_seconds)).push(stretch, each)
    })?;
    let Some(parts) = parts else {
        return Err(AudioError::Format("The file has no audio in it".into()));
    };
    parts.finish(ended, known_frames, each)
}
