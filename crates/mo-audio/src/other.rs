//! MP3, FLAC, M4A (AAC), OGG (Vorbis) and AAC, decoded with symphonia on every system. The C# app needed Windows'
//! decoders for these, so a Mac or Linux left them out.

use std::fs::File;
use std::io::{Cursor, ErrorKind, Read, Seek, SeekFrom};
use std::panic::AssertUnwindSafe;
use std::path::Path;

use symphonia::core::codecs::audio::well_known::{CODEC_ID_FLAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::TimeBase;

use crate::parts::{Each, Ended, Parts};
use crate::{AudioData, AudioError};

/// The kinds symphonia decodes here, by extension.
pub const OTHER_EXTENSIONS: [&str; 5] = ["mp3", "flac", "m4a", "ogg", "aac"];

/// Whether this file is one of [`OTHER_EXTENSIONS`] (ignoring case).
pub fn is_other(path: &Path) -> bool {
    crate::read::has_extension(path, &OTHER_EXTENSIONS)
}

/// Up to `max_seconds` from the start of an MP3, FLAC, M4A, OGG or AAC file. A damaged stretch part way in ends the
/// read there with what came before it (a frame its decoder refuses is silence of its length instead, and so is a
/// FLAC frame lost to damage: [`each_stretch`]); a decoder that falls over on a damaged file gives an error, never a
/// crash.
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
    mo_catch::catch(AssertUnwindSafe(|| {
        let chained = ogg_chained(&mut &file);
        decode_parts(probe(file, path)?, part_seconds, chained, each)
    }))
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
    mo_catch::catch(AssertUnwindSafe(|| {
        let chained = ogg_chained(&mut Cursor::new(&stream[..]));
        decode_format(probe_ogg(stream)?, max_seconds, from_frame, chained)
    }))
    .unwrap_or_else(|_| Err(AudioError::Format("The decoder couldn't read this file (it may be damaged)".into())))
}

/// The whole of an Ogg Vorbis stream held in memory (a WAV file's: [`read_ogg_bytes`]), handed to `each` a part of up
/// to `part_seconds` at a time ([`crate::read_parts`]). Damage is handled as [`read_other`] handles it. Its whole
/// length in seconds.
pub(crate) fn read_ogg_bytes_parts(stream: Vec<u8>, part_seconds: f64, each: &mut Each) -> Result<f64, AudioError> {
    mo_catch::catch(AssertUnwindSafe(|| {
        let chained = ogg_chained(&mut Cursor::new(&stream[..]));
        decode_parts(probe_ogg(stream)?, part_seconds, chained, each)
    }))
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
    let chained = ogg_chained(&mut &file);
    decode_format(probe(file, path)?, max_seconds, 0, chained)
}

/// How far from an Ogg file's end its last page is looked for.
const LAST_PAGE_ROOM: u64 = 64 * 1024;

/// Whether an Ogg file's last page belongs to another stream than its first: streams joined one after another (a
/// chained Ogg file, as a radio recording is), so the length its first stream says isn't the file's. False for any other
/// file. Leaves `source` at its start.
fn ogg_chained(source: &mut (impl Read + Seek)) -> bool {
    let mut chained = || {
        let mut head = [0u8; 27];
        source.seek(SeekFrom::Start(0)).ok()?;
        source.read_exact(&mut head).ok()?;
        let first = page_serial(&head)?;
        let len = source.seek(SeekFrom::End(0)).ok()?;
        source.seek(SeekFrom::Start(len.saturating_sub(LAST_PAGE_ROOM))).ok()?;
        let mut tail = Vec::new();
        source.by_ref().take(LAST_PAGE_ROOM).read_to_end(&mut tail).ok()?;
        Some(last_page_serial(&tail)? != first)
    };
    let chained = chained().unwrap_or(false);
    let _ = source.seek(SeekFrom::Start(0));
    chained
}

/// Which stream an Ogg page belongs to (its serial number), when `page` starts with a page's header.
fn page_serial(page: &[u8]) -> Option<u32> {
    let head = page.first_chunk::<27>()?;
    (head.starts_with(b"OggS") && head[4] == 0).then(|| u32::from_le_bytes([head[14], head[15], head[16], head[17]]))
}

/// The serial number of the last whole Ogg page in `tail`, a file's end (a page cut off by the file's end, or bytes
/// after the last page, are passed over).
fn last_page_serial(tail: &[u8]) -> Option<u32> {
    (0..tail.len()).rev().find_map(|at| {
        let page = tail.get(at..)?;
        let serial = page_serial(page)?;
        let segments = usize::from(page[26]);
        let body: usize = page.get(27..27 + segments)?.iter().map(|&n| usize::from(n)).sum();
        (27 + segments + body <= page.len()).then_some(serial)
    })
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
    /// What its packets' times count in, when the file says (else they count in frames), and its sample rate as the
    /// file says it.
    time_base: Option<TimeBase>,
    rate: Option<u32>,
    /// What its packets hold.
    coded: Coded,
    decoder: Box<dyn AudioDecoder>,
}

/// What a track's packets hold, for how long one its decoder refuses is ([`Stream::length_of`]).
#[derive(Clone, Copy, PartialEq)]
enum Coded {
    /// MPEG audio: MP3 (and MP1 and MP2).
    Mpeg,
    /// FLAC.
    Flac,
    /// AAC, Vorbis and the rest.
    Other,
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
    let coded = match params.codec {
        CODEC_ID_MP1 | CODEC_ID_MP2 | CODEC_ID_MP3 => Coded::Mpeg,
        CODEC_ID_FLAC => Coded::Flac,
        _ => Coded::Other,
    };
    Ok(AudioTrack {
        id: track.id,
        frames: track.num_frames,
        time_base: track.time_base,
        rate: params.sample_rate,
        coded,
        decoder,
    })
}

/// The most frames a packet holds (FLAC's biggest block), whatever a damaged file says its length is.
const MOST_IN_A_PACKET: u64 = 65_536;

/// An MPEG audio frame's version and layer, and its sample rate, as its header says them (a packet's first three
/// bytes). None for bytes that aren't a frame's header.
fn mpeg_kind(data: &[u8]) -> Option<(u8, u8)> {
    match *data {
        [0xFF, b1, b2, ..] if b1 & 0xE0 == 0xE0 => Some((b1 & 0x1E, b2 & 0x0C)),
        _ => None,
    }
}

/// How many frames an MPEG audio frame holds, by its header: 384 for layer I, 1,152 for layer II and MPEG-1 layer III,
/// 576 for MPEG-2 and 2.5 layer III.
fn mpeg_frames(data: &[u8]) -> Option<u64> {
    let (version_layer, _) = mpeg_kind(data)?;
    match (version_layer >> 3 & 3, version_layer >> 1 & 3) {
        (_, 0b11) => Some(384),
        (_, 0b10) | (0b11, 0b01) => Some(1152),
        (0b00 | 0b10, 0b01) => Some(576),
        _ => None,
    }
}

/// The stream [`each_stretch`] is going through: the file's first, or one joined on after it.
struct Stream {
    /// The id its packets carry.
    id: u32,
    /// What its packets' times count in (frames when the file doesn't say), and its sample rate as the file says it.
    time_base: Option<TimeBase>,
    told_rate: Option<u32>,
    /// How many frames the file says it has, when it says.
    frames: Option<u64>,
    coded: Coded,
    /// None for a stream joined on that this app doesn't decode (Opus), or whose sound doesn't match the first one's:
    /// it's counted by its packets' lengths instead.
    decoder: Option<Box<dyn AudioDecoder>>,
    /// MPEG: the version, layer and sample rate its first packet's header says ([`mpeg_kind`]).
    kind: Option<(u8, u8)>,
    /// FLAC: where its next packet starts, in frames.
    next: Option<u64>,
}

impl Stream {
    fn of(track: AudioTrack) -> Stream {
        Stream {
            id: track.id,
            time_base: track.time_base,
            told_rate: track.rate,
            frames: track.frames,
            coded: track.coded,
            decoder: Some(track.decoder),
            kind: None,
            next: (track.coded == Coded::Flac).then_some(0),
        }
    }

    /// A stream joined on that's counted, not decoded (one with no decoder here, as Opus). None when it has no audio.
    fn counted(format: &dyn FormatReader) -> Option<Stream> {
        let track = format.default_track(TrackType::Audio)?;
        Some(Stream {
            id: track.id,
            time_base: track.time_base,
            told_rate: track.codec_params.as_ref().and_then(|p| p.audio()).and_then(|p| p.sample_rate),
            frames: track.num_frames,
            coded: Coded::Other,
            decoder: None,
            kind: None,
            next: None,
        })
    }

    /// The longest a packet of it counts, in its time units: [`MOST_IN_A_PACKET`] frames at `rate` (the sound's, else
    /// what the file says), so a length a damaged file gives can't make a long silence.
    fn most(&self, rate: u32) -> u64 {
        let rate = if rate != 0 { rate } else { self.told_rate.unwrap_or(0) };
        match self.time_base {
            Some(tb) if rate != 0 => {
                let units = u128::from(MOST_IN_A_PACKET) * u128::from(tb.denom.get())
                    / (u128::from(tb.numer.get()) * u128::from(rate));
                u64::try_from(units).unwrap_or(u64::MAX).max(1)
            }
            _ => MOST_IN_A_PACKET,
        }
    }

    /// How long a packet of it is, in its time units, when it isn't decoded (refused, or counted). An MPEG frame goes
    /// by its header less the encoder's delay and padding: symphonia's MP3 reader leaves them in a packet's length
    /// (0.6.1's `trimmed_dur` takes off trims that are still nought), and its Ogg reader leaves them out. Nothing for
    /// an MPEG "frame" whose header says another version, layer or sample rate than the stream's: after the first
    /// frame, the reader takes any bytes that pass for a header as a frame (a picture in the tag of a second MP3 joined
    /// on), and those were never sound. A frame that goes mono in a stereo stream still counts.
    fn length_of(&self, packet: &Packet) -> u64 {
        if self.coded != Coded::Mpeg {
            return packet.dur.get();
        }
        let ours = mpeg_kind(&packet.data).is_some_and(|kind| Some(kind) == self.kind);
        let frames = mpeg_frames(&packet.data).filter(|_| ours).unwrap_or(0);
        frames.saturating_sub(packet.trim_start.get()).saturating_sub(packet.trim_end.get())
    }
}

/// How many frames `units` of a stream's time are, at `rate` frames a second (frames already when the file doesn't
/// say what its times count in).
fn frames_in(units: u64, time_base: Option<TimeBase>, rate: u32) -> u64 {
    match time_base {
        Some(tb) => {
            let frames = u128::from(units) * u128::from(tb.numer.get()) * u128::from(rate) / u128::from(tb.denom.get());
            u64::try_from(frames).unwrap_or(u64::MAX)
        }
        None => units,
    }
}

/// Hands `frames` of silence to `take`, `count` channels at `rate`, a packet's worth at a time ([`each_stretch`]).
/// Once `take` says to stop, the frames of it it wasn't given.
fn silence(rate: u32, count: usize, frames: u64, take: &mut dyn FnMut(u32, &[Vec<f32>]) -> bool) -> Option<u64> {
    let mut left = frames;
    while left > 0 {
        let n = left.min(MOST_IN_A_PACKET);
        let piece = vec![vec![0.0f32; n as usize]; count];
        left -= n;
        if !take(rate, &piece) {
            return Some(left);
        }
    }
    None
}

/// How [`each_stretch`] went through a file.
struct Through {
    ended: Ended,
    /// Once `take` said to stop, the frames after the last it was given, counted without decoding them when that was
    /// asked; nought otherwise.
    rest: u64,
}

/// Goes through a probed file's audio (`track`) from its start, handing each stretch its decoder gives to `take`: its
/// sample rate, and its samples, a channel each, as long as each other. Until the file ends, damage ends it (what came
/// before is kept), or `take` says to stop (false); then, with `count_rest` (a file that doesn't say how long it is),
/// the rest of it is counted from its packets' lengths, without decoding them.
///
/// What the decoder doesn't give is silence of its length, so what comes after stays in its place, as players keep
/// their time, and the length comes out right: a frame it refuses (damaged on its own, or each one after an MP3's
/// sound changes part way, mono to stereo), and a FLAC frame lost to damage (one whose checksum fails never comes as a
/// packet, so it's found as a jump in the packets' times). That's to the frame for MP3, FLAC and AAC, and near enough
/// for Vorbis (its blocks overlap, so the one after a refused one can come out up to a few hundred frames longer or
/// shorter). Damage the readers pass over themselves (a bad Ogg page, an MP3 frame whose header is spoilt) still comes
/// early.
///
/// Another stream joined on (a chained Ogg file: two joined into one) is gone on into with a decoder of its own, as
/// players do. One whose sound doesn't match the first one's (another sample rate or channels), or with no decoder here
/// (Opus), is silence as long as it is: this read keeps one sound. Damage before any sound, or a real read failure (a
/// drive gone), is an error rather than a part passed off as the whole file. Both [`decode_format`] and
/// [`decode_parts`] go through a file this way, so they read it the same.
fn each_stretch(
    format: &mut dyn FormatReader,
    track: AudioTrack,
    count_rest: bool,
    take: &mut dyn FnMut(u32, &[Vec<f32>]) -> bool,
) -> Result<Through, AudioError> {
    let mut stream = Stream::of(track);
    let mut rate = 0u32;
    let mut count = 0usize;
    let mut scratch: Vec<Vec<f32>> = Vec::new();
    // Silence owed since the last stretch, in the stream's time units: handed on before the next one, at the sound's
    // rate once that's known.
    let mut owed = 0u64;
    // Frames `take` wasn't given when it said to stop.
    let mut not_given = 0u64;
    let ended = loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break Ended::End,
            // The first page of a stream joined on: its packets come with an id and decoder of their own. What's owed
            // goes before it, as it belongs to the stream before (with no sound yet there's no rate for it, and it
            // goes).
            Err(Error::ResetRequired) => {
                if rate != 0
                    && let Some(left) = silence(rate, count, frames_in(owed, stream.time_base, rate), take)
                {
                    owed = 0;
                    not_given = left;
                    break Ended::Stopped;
                }
                owed = 0;
                stream = match audio_track(format) {
                    Ok(next) => Stream::of(next),
                    Err(_) if rate != 0 => match Stream::counted(format) {
                        Some(counted) => counted,
                        None => break Ended::End,
                    },
                    Err(e) => return Err(e),
                };
                continue;
            }
            Err(Error::DecodeError(_)) if rate != 0 => break Ended::Damage,
            Err(Error::IoError(e)) if rate != 0 && e.kind() == ErrorKind::UnexpectedEof => break Ended::Damage,
            Err(e) => return Err(format_error(e)),
        };
        if packet.track_id != stream.id {
            continue;
        }
        let most = stream.most(rate);
        // FLAC: a jump in the packets' times is frames lost, up to where the file says it ends (one frame's worth when
        // it doesn't say).
        if let Some(next) = stream.next {
            let starts = u64::try_from(packet.pts.get()).unwrap_or(0);
            let room = stream.frames.map_or(MOST_IN_A_PACKET, |all| all.saturating_sub(next));
            owed = owed.saturating_add(starts.saturating_sub(next).min(room));
            stream.next = Some(starts.max(next).saturating_add(packet.dur.get()));
        }
        if stream.coded == Coded::Mpeg && stream.kind.is_none() {
            stream.kind = mpeg_kind(&packet.data);
        }
        let length = stream.length_of(&packet).min(most);
        let Some(decoder) = stream.decoder.as_mut() else {
            owed = owed.saturating_add(length);
            continue;
        };
        let buf = match decoder.decode(&packet) {
            Ok(buf) => buf,
            // A frame the decoder refuses. One whose bits run out comes as an I/O error (the packet is already in
            // memory, so that's damage too), and once the sound has come, damage in an AAC frame can read as a tool
            // this decoder doesn't have.
            Err(Error::DecodeError(_) | Error::IoError(_)) => {
                owed = owed.saturating_add(length);
                continue;
            }
            Err(Error::Unsupported(_)) if rate != 0 => {
                owed = owed.saturating_add(length);
                continue;
            }
            Err(e) => return Err(format_error(e)),
        };
        let (stretch_rate, channels) = (buf.spec().rate(), buf.spec().channels().count().max(1));
        if rate == 0 {
            (rate, count) = (stretch_rate, channels);
        } else if stretch_rate != rate || channels != count {
            // The sound changes part way (a stream joined on at another rate, or with other channels): the rest of
            // that stream is counted, not decoded.
            stream.decoder = None;
            owed = owed.saturating_add(length);
            continue;
        }
        buf.copy_to_vecs_planar(&mut scratch);
        let frames = scratch.first().map_or(0, Vec::len) as u64;
        if owed > 0
            && let Some(left) = silence(rate, count, frames_in(std::mem::take(&mut owed), stream.time_base, rate), take)
        {
            not_given = left + frames;
            break Ended::Stopped;
        }
        if !take(rate, &scratch) {
            break Ended::Stopped;
        }
    };
    if ended == Ended::Stopped {
        let rest = if count_rest {
            not_given + frames_in(owed, stream.time_base, rate) + rest_of(format, stream, rate)
        } else {
            0
        };
        return Ok(Through { ended, rest });
    }
    // Owed at the end is silence too, so the sound keeps its length. That's what came, as the read went to the end: the
    // length the file says is no surer (for an MP3 without a header saying it, it's worked out from its size).
    if rate != 0
        && let Some(left) = silence(rate, count, frames_in(owed, stream.time_base, rate), take)
    {
        return Ok(Through { ended: Ended::Stopped, rest: if count_rest { left } else { 0 } });
    }
    Ok(Through { ended, rest: 0 })
}

/// How many frames a file has left after a read stopped part way in `stream`, at the sound's `rate`: counted from its
/// packets' lengths, without decoding them ([`each_stretch`]'s `count_rest`).
fn rest_of(format: &mut dyn FormatReader, mut stream: Stream, rate: u32) -> u64 {
    let (mut frames, mut units) = (0u64, 0u64);
    loop {
        match format.next_packet() {
            Ok(Some(packet)) if packet.track_id == stream.id => {
                units = units.saturating_add(stream.length_of(&packet).min(stream.most(rate)));
            }
            Ok(Some(_)) => {}
            Err(Error::ResetRequired) => {
                frames = frames.saturating_add(frames_in(std::mem::take(&mut units), stream.time_base, rate));
                match Stream::counted(format) {
                    Some(next) => stream = next,
                    None => break,
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    frames.saturating_add(frames_in(units, stream.time_base, rate))
}

/// Decodes up to `max_seconds` of a probed file's audio, leaving out its first `skip` frames. `chained`: streams joined
/// one after another ([`ogg_chained`]), so the length the file says is only its first stream's.
fn decode_format(
    mut format: Box<dyn FormatReader>,
    max_seconds: f64,
    skip: u64,
    chained: bool,
) -> Result<AudioData, AudioError> {
    let mut skip = usize::try_from(skip).unwrap_or(usize::MAX);
    let mut skipped = 0u64;
    let track = audio_track(format.as_ref())?;
    let known_frames = track.frames.filter(|_| !chained);
    let mut sample_rate = 0u32;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut wanted = usize::MAX;
    // Every frame handed on, kept or not.
    let mut given = 0u64;
    let through = each_stretch(format.as_mut(), track, known_frames.is_none(), &mut |rate, stretch| {
        if channels.is_empty() {
            sample_rate = rate;
            // `as` saturates: NaN and anything below zero give 0, infinity gives every frame. Never more than one
            // read may hold, as the WAV reader.
            let most = usize::try_from(crate::read::MOST_BYTES).unwrap_or(usize::MAX) / (4 * stretch.len());
            wanted = ((max_seconds * f64::from(rate)) as usize).min(most);
            let room = known_frames.map_or(0, |k| usize::try_from(k).unwrap_or(usize::MAX)).min(wanted);
            channels = (0..stretch.len()).map(|_| Vec::with_capacity(room)).collect();
        }
        given += stretch.first().map_or(0, Vec::len) as u64;
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
    // The whole length: what was decoded when it was all decoded (to its end or to damage); else what the file says,
    // or when it doesn't say, all that was handed on and the rest counted.
    let total = match through.ended {
        Ended::End | Ended::Damage => decoded,
        Ended::Stopped => known_frames.unwrap_or(given + through.rest),
    };
    let total = total.max(decoded);
    Ok(AudioData { sample_rate, file_seconds: total as f64 / f64::from(sample_rate), channels, ..AudioData::default() })
}

/// The whole of a probed file's audio, handed to `each` a part of up to `part_seconds` at a time, in order, with its
/// sample rate ([`read_other_parts`]); `each` says whether to go on. The file is gone through as [`decode_format`]
/// goes through it ([`each_stretch`]); `chained` as there. The whole length in seconds: what was decoded, or when
/// damage or `each` ended it early, what the file says if it's more, so a clip that plays the whole file still plays it
/// at its speed.
fn decode_parts(
    mut format: Box<dyn FormatReader>,
    part_seconds: f64,
    chained: bool,
    each: &mut Each,
) -> Result<f64, AudioError> {
    let track = audio_track(format.as_ref())?;
    let known_frames = track.frames.filter(|_| !chained);
    let mut parts: Option<Parts> = None;
    let through = each_stretch(format.as_mut(), track, false, &mut |rate, stretch| {
        parts.get_or_insert_with(|| Parts::new(rate, stretch.len(), part_seconds)).push(stretch, each)
    })?;
    let Some(parts) = parts else {
        return Err(AudioError::Format("The file has no audio in it".into()));
    };
    parts.finish(through.ended, known_frames, each)
}

#[cfg(test)]
mod tests {
    use symphonia::core::units::{Duration, Timestamp};

    use super::*;

    /// A stream of the test tone's kind of MP3: MPEG-2 layer III at 22,050 a second.
    fn mp3_stream() -> Stream {
        Stream {
            id: 0,
            time_base: None,
            told_rate: Some(22_050),
            frames: None,
            coded: Coded::Mpeg,
            decoder: None,
            kind: mpeg_kind(&[0xFF, 0xF3, 0x80, 0x00]),
            next: None,
        }
    }

    /// A packet with this header, `dur` long as its reader says, and these trims.
    fn packet(header: [u8; 4], dur: u64, trims: (u64, u64)) -> Packet {
        let mut packet = Packet::new(0, Timestamp::ZERO, Duration::new(dur), header.to_vec());
        (packet.trim_start, packet.trim_end) = (Duration::new(trims.0), Duration::new(trims.1));
        packet
    }

    #[test]
    fn mpeg_frames_hold_as_many_frames_as_their_header_says() {
        // Layer I, layer II, MPEG-1 layer III, MPEG-2 and 2.5 layer III; a reserved layer.
        let frames = |b1: u8| mpeg_frames(&[0xFF, b1, 0x90, 0x00]);
        assert_eq!(
            [0xFF, 0xFD, 0xFB, 0xF3, 0xE3, 0xF9].map(frames),
            [Some(384), Some(1152), Some(1152), Some(576), Some(576), None]
        );
        assert_eq!(mpeg_kind(b"ID3"), None);
    }

    /// The code review of SN-18's fix: after an MP3's first frame, symphonia's reader takes any bytes that pass for a
    /// frame's header as a frame (a picture in the tag of a second MP3 joined on), and the decoder refuses them. They
    /// were never sound, so they count for nothing; a frame of the stream's own kind counts by its header, less the
    /// encoder's delay and padding, whatever length the reader gives it.
    #[test]
    fn a_refused_mpeg_frame_counts_only_when_its_header_is_the_streams_kind() {
        let stream = mp3_stream();
        // The stream's own kind, gone mono: a whole frame, less its trims, whether the reader leaves them in its length
        // (symphonia 0.6.1) or takes them off.
        assert_eq!(stream.length_of(&packet([0xFF, 0xF3, 0x80, 0xC0], 576, (0, 0))), 576);
        assert_eq!(stream.length_of(&packet([0xFF, 0xF3, 0x82, 0x00], 576, (100, 76))), 400);
        assert_eq!(stream.length_of(&packet([0xFF, 0xF3, 0x82, 0x00], 400, (100, 76))), 400);
        // Another version (MPEG-1), layer (II) or sample rate (24,000 a second), or no header at all: nothing.
        for header in [[0xFF, 0xFB, 0x90, 0x00], [0xFF, 0xF5, 0x80, 0x00], [0xFF, 0xF3, 0x84, 0x00], [0; 4]] {
            assert_eq!(stream.length_of(&packet(header, 576, (0, 0))), 0, "{header:02X?}");
        }
        // Other kinds of audio go by the length their reader gives.
        let ogg = Stream { coded: Coded::Other, kind: None, ..mp3_stream() };
        assert_eq!(ogg.length_of(&packet([0; 4], 546, (10, 20))), 546);
    }
}
