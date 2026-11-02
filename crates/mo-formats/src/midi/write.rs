//! Writes Standard MIDI Files (.mid), as the C# app's `MidiWriter` does, from the same notes the readers give.
//!
//! Inside the app a note keeps MIDI 2.0's 16-bit velocity and a controller its 32 bits (rule 22). A Standard MIDI File
//! is MIDI 1.0, so only here, while the file's bytes are made, are they cut down to 7 bits (14 for pitch bend):
//!
//! - **Velocity**: [`crate::velocity::to_7`], the nearest MIDI 1.0 velocity (never 0, which MIDI 1.0 reads as the note
//!   ending). The reader scales it back up with [`crate::velocity::from_7`], MIDI 2.0's rule, and `to_7` gives
//!   every value from 1 to 127 back as it was, so writing what was read gives the same bytes again.
//! - **Controllers** (and pressure, and pitch bend): MIDI 2.0's own rule for fewer bits, which is to drop the low bits
//!   ([`controller_to_7`], [`controller_to_14`]). That is the exact undoing of [`crate::scale_up`], which the MIDI
//!   Clip reader uses to scale MIDI 1.0 values up, so these round-trip too.
//!
//! What the file looks like:
//!
//! - Format 1 (the C# app's): a first track with the song's name (and its tempo and time signature when asked for),
//!   then a track per instrument. Format 0 (one track holding it all) when asked for, for a song of one track.
//! - **No tempo** unless [`WriteOptions::tempo`] is set: a DAW asks whether to take a MIDI file's tempo for the
//!   project (Bitwig does every time), and the app's MIDI never should (spec 3.7 and 4.4, asked for on 2026-10-18).
//!   The notes are in beats, so they land in the same places at any tempo.
//! - **No running status**: every event carries its own status byte. Running status only saves a byte an event,
//!   which doesn't matter for a clip, and some readers lose their place with it (after a track name or a lyric, say);
//!   with it off, every note-off is a real note-off (0x80), never a note-on at velocity 0.
//! - At the same tick, a note's end comes before the next note's start, so repeated notes never cut each other.
//!
//! What can't be written is an error, never a panic: a key or channel MIDI 1.0 doesn't have, a negative time, a tick
//! too far for the file. Notes of no length and notes over each other on the same key are fixed (or refused, if
//! [`WriteOptions::tidy`] says so), because a MIDI 1.0 file can't hold them in a way every reader agrees on.

use std::collections::HashMap;
use std::fmt;

use super::{Controller, ControllerKind, MidiPart};
use crate::model::{MidiClip, NoteEvent};

/// MIDI 1.0's 16 channels.
const CHANNELS: u8 = 16;
/// The drum channel (channel 10, counted from 1), which General MIDI keeps for drums.
const DRUM_CHANNEL: u8 = 9;
/// The biggest gap between two events a Standard MIDI File can hold: four bytes of seven bits.
const MAX_DELTA: i64 = 0x0FFF_FFFF;
/// Text events are cut to this many bytes, as the C# writer does (at a whole letter here).
const MAX_TEXT: usize = 255;

/// A song to write: a name, its ticks a beat, and its tracks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmfSong {
    /// Written as the first track's name (format 1), or the only track's when it has none (format 0).
    pub name: String,
    /// Ticks a beat (a quarter note), 1 to 32767.
    pub ppq: u16,
    /// Beats a minute. Only written when [`WriteOptions::tempo`] is set.
    pub tempo: Option<f64>,
    /// Written when given.
    pub time_signature: Option<TimeSignature>,
    pub tracks: Vec<SmfTrack>,
    /// Where the song ends, in ticks, when it goes on past its last note (a loop with empty beats at its end): each
    /// instrument track ends there, so a DAW makes the clip that long.
    pub end: Option<i64>,
}

/// One instrument's track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmfTrack {
    pub name: String,
    /// The MIDI channel, 0 to 15 (9 is General MIDI's drums).
    pub channel: u8,
    /// The General MIDI sound (0 to 127) to set at the start, if any.
    pub program: Option<u8>,
    /// In ticks at the song's PPQ, in any order. A note's muted and step marks and its MIDI 2.0 attribute have no
    /// place in a MIDI 1.0 file and are left out; muted notes are written, as the C# writer writes them.
    pub notes: Vec<NoteEvent>,
    /// In order. The kinds MIDI 1.0 has ([`fits_midi1`]) are written; MIDI 2.0's per-note and relative controllers
    /// have no MIDI 1.0 message and are left out.
    pub controllers: Vec<Controller>,
}

/// A time signature, such as 6/8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSignature {
    /// Beats a bar, 1 or more.
    pub numerator: u8,
    /// The note a beat is: 1, 2, 4, 8, 16, 32, 64 or 128.
    pub denominator: u8,
}

/// Which Standard MIDI File format to write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SmfFormat {
    /// Format 0: one track holding everything. Only for a song of at most one track.
    Zero,
    /// Format 1: a first track with the song's name, then a track per instrument (the C# writer's).
    #[default]
    One,
    /// Format 0 for a song of at most one track, format 1 otherwise.
    Auto,
}

/// What to do with notes a MIDI 1.0 file can't hold as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tidy {
    /// A note of no length gets one tick (as the reader reads a note's end on its start). A note still sounding when
    /// the same key starts again on its channel ends there, which is what a MIDI 1.0 instrument does anyway; two
    /// notes starting together on the same key become one, the longer and louder of the two.
    #[default]
    Fix,
    /// Either of those is an error instead.
    Refuse,
}

/// How to write the file. The default is the C# writer's: format 1, no tempo, notes fixed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteOptions {
    pub format: SmfFormat,
    /// Write the song's tempo. Off by default: see the top of this file.
    pub tempo: bool,
    pub tidy: Tidy,
}

/// A written file, and what MIDI 1.0 made it leave out, so the caller can say so (the file is written anyway: a
/// drag or an export shouldn't fail over it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    /// The Standard MIDI File's bytes.
    pub bytes: Vec<u8>,
    /// MIDI 2.0's per-note and relative controllers left out: MIDI 1.0 has no message for them.
    pub dropped_controllers: usize,
    /// Tracks (by place in [`SmfSong::tracks`]) that had to share a channel with an earlier track of another sound,
    /// so they play with that sound: more than 15 different sounds besides drums.
    pub shared_channels: Vec<usize>,
}

/// Why a song couldn't be written. `track` is the track's place in [`SmfSong::tracks`] (from 0), whichever format
/// is written.
#[derive(Clone, Debug, PartialEq)]
pub enum WriteError {
    /// Ticks a beat must be 1 to 32767 (the top bit means SMPTE time, which this writer doesn't write).
    BadPpq(u16),
    /// Format 0 has room for one track only.
    TooManyTracksForFormat0(usize),
    /// A Standard MIDI File holds at most 65535 tracks.
    TooManyTracks(usize),
    /// The tempo isn't a number of beats a minute a file can hold.
    BadTempo(f64),
    BadTimeSignature(TimeSignature),
    /// MIDI 1.0 has channels 0 to 15 only.
    ChannelOutOfRange {
        track: usize,
        channel: u8,
    },
    /// MIDI 1.0 has sounds 0 to 127 only.
    ProgramOutOfRange {
        track: usize,
        program: u8,
    },
    /// MIDI 1.0 has keys 0 to 127 only.
    KeyOutOfRange {
        track: usize,
        key: u8,
    },
    /// A controller number, key or bank over 127.
    ControllerOutOfRange {
        track: usize,
        kind: ControllerKind,
    },
    /// Something starts before the song does.
    NegativeTime {
        track: usize,
        tick: i64,
    },
    /// A note ends before it starts.
    NegativeLength {
        track: usize,
        start: i64,
        length: i64,
    },
    /// A note of no length, with [`Tidy::Refuse`].
    ZeroLength {
        track: usize,
        start: i64,
        key: u8,
    },
    /// A note still sounding when the same key starts again, with [`Tidy::Refuse`].
    Overlap {
        track: usize,
        start: i64,
        key: u8,
    },
    /// An event too far from the one before it for a file to hold (over 268,435,455 ticks).
    TooFar {
        track: usize,
        tick: i64,
    },
    /// A track's bytes are more than a file can say (4 GB).
    TrackTooLong {
        track: usize,
    },
    /// Parts at different ticks a beat can't share a file (see [`SmfSong::from_parts`]).
    MixedPpq {
        first: u16,
        other: u16,
    },
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::BadPpq(ppq) => write!(f, "{ppq} ticks a beat can't be written (1 to 32767)"),
            WriteError::TooManyTracksForFormat0(n) => write!(f, "a format 0 MIDI file holds one track, not {n}"),
            WriteError::TooManyTracks(n) => write!(f, "a MIDI file holds at most 65535 tracks, not {n}"),
            WriteError::BadTempo(bpm) => write!(f, "a tempo of {bpm} beats a minute can't be written"),
            WriteError::BadTimeSignature(t) => {
                write!(f, "a time signature of {}/{} can't be written", t.numerator, t.denominator)
            }
            WriteError::ChannelOutOfRange { track, channel } => {
                write!(f, "track {track}: channel {channel} isn't one of MIDI's 16 (0 to 15)")
            }
            WriteError::ProgramOutOfRange { track, program } => {
                write!(f, "track {track}: sound {program} isn't one of MIDI's 128")
            }
            WriteError::KeyOutOfRange { track, key } => write!(f, "track {track}: key {key} is over MIDI's 127"),
            WriteError::ControllerOutOfRange { track, kind } => {
                write!(f, "track {track}: controller {kind:?} has a number over MIDI's 127")
            }
            WriteError::NegativeTime { track, tick } => write!(f, "track {track}: something starts at tick {tick}"),
            WriteError::NegativeLength { track, start, length } => {
                write!(f, "track {track}: the note at tick {start} is {length} ticks long")
            }
            WriteError::ZeroLength { track, start, key } => {
                write!(f, "track {track}: the note on key {key} at tick {start} has no length")
            }
            WriteError::Overlap { track, start, key } => {
                write!(f, "track {track}: key {key} starts at tick {start} while it's still sounding")
            }
            WriteError::TooFar { track, tick } => write!(f, "track {track}: tick {tick} is too far for a MIDI file"),
            WriteError::TrackTooLong { track } => write!(f, "track {track} is too long for a MIDI file"),
            WriteError::MixedPpq { first, other } => {
                write!(f, "parts at {first} and {other} ticks a beat can't share a MIDI file")
            }
        }
    }
}

impl std::error::Error for WriteError {}

/// A 32-bit controller value as MIDI 1.0's 7 bits, by MIDI 2.0's rule: the low 25 bits are dropped. It undoes
/// [`crate::scale_up`]`(v, 7, 32)` exactly.
pub fn controller_to_7(value: u32) -> u8 {
    (value >> 25) as u8
}

/// A 32-bit value as MIDI 1.0's 14 bits (pitch bend, and a registered controller's value), by the same rule. The
/// middle, `0x8000_0000`, becomes 8192, MIDI 1.0's "no bend".
pub fn controller_to_14(value: u32) -> u16 {
    (value >> 18) as u16
}

/// Whether a controller kind has a MIDI 1.0 message, so a Standard MIDI File can hold it.
pub fn fits_midi1(kind: ControllerKind) -> bool {
    matches!(
        kind,
        ControllerKind::Control(_)
            | ControllerKind::PitchBend
            | ControllerKind::ChannelPressure
            | ControllerKind::KeyPressure(_)
            | ControllerKind::Registered(..)
            | ControllerKind::Assignable(..)
    )
}

impl SmfSong {
    /// A clip's tracks as a song, as the C# writer lays them out: each track on its own channel in order, skipping
    /// channel 10 (General MIDI's drums, which a clip doesn't mark), and round again after 15 (a clip's tracks set no
    /// sound, so sharing a channel loses nothing). Its notes are taken as they are: [`write_clip`] lengthens FL's
    /// steps first.
    pub fn from_clip(clip: &MidiClip) -> SmfSong {
        let mut channels = melodic_channels();
        SmfSong {
            name: clip.name.clone(),
            ppq: clip.ppq,
            tempo: None,
            time_signature: None,
            tracks: clip
                .tracks
                .iter()
                .map(|(name, notes)| SmfTrack {
                    name: name.clone(),
                    channel: channels.next().unwrap_or_default(),
                    program: None,
                    notes: notes.clone(),
                    controllers: Vec::new(),
                })
                .collect(),
            end: clip.end,
        }
    }

    /// Parts a MIDI reader found as a song, a track each. Each keeps its track's own name, its sound and its
    /// controllers; the song takes the first part's tempo. Parts at different ticks a beat are an error.
    ///
    /// A part doesn't say which channel it was on, nor where its clip ended (the readers don't keep either), so the
    /// channels are handed out again and the tracks end at their last note:
    /// - drum parts go on channel 10;
    /// - the others take the other 15 channels in order, a channel each while there are enough;
    /// - with more than 15, parts with the same sound share a channel (a channel plays one sound at a time); past 15
    ///   different sounds, channels are shared round again and [`Written::shared_channels`] says which tracks lost
    ///   their sound.
    pub fn from_parts(name: &str, parts: &[MidiPart]) -> Result<SmfSong, WriteError> {
        let ppq = parts.first().map_or(96, |p| p.ppq);
        if let Some(other) = parts.iter().find(|p| p.ppq != ppq) {
            return Err(WriteError::MixedPpq { first: ppq, other: other.ppq });
        }
        let melodic = parts.iter().filter(|p| !p.drums).count();
        let per_sound = melodic > usize::from(CHANNELS - 1);
        let mut channels = melodic_channels();
        // With too many parts: the channel each sound got (a part with no sound counts as a sound of its own).
        let mut by_sound: Vec<(Option<u8>, u8)> = Vec::new();
        let mut channel_for = |p: &MidiPart| {
            if p.drums {
                return DRUM_CHANNEL;
            }
            if !per_sound {
                return channels.next().unwrap_or_default();
            }
            if let Some(&(_, ch)) = by_sound.iter().find(|(sound, _)| *sound == p.program) {
                return ch;
            }
            let ch = channels.next().unwrap_or_default();
            by_sound.push((p.program, ch));
            ch
        };
        let tracks = parts
            .iter()
            .map(|p| SmfTrack {
                name: if p.track_name.is_empty() { p.name.clone() } else { p.track_name.clone() },
                channel: channel_for(p),
                program: p.program,
                notes: p.notes.clone(),
                controllers: p.controllers.clone(),
            })
            .collect();
        Ok(SmfSong {
            name: name.to_owned(),
            ppq,
            tempo: parts.iter().find_map(|p| p.tempo),
            time_signature: None,
            tracks,
            end: None,
        })
    }
}

/// Channels 0 to 15 without the drum channel, over and over.
fn melodic_channels() -> impl Iterator<Item = u8> {
    (0..CHANNELS).filter(|&c| c != DRUM_CHANNEL).cycle()
}

/// A clip as a MIDI file, as the C# `MidiWriter.Write` makes it: format 1, no tempo, notes fixed. FL's
/// step-sequencer steps (which have no length) each play a sixteenth, or up to the next note on their key or the
/// clip's end if that's sooner, as the MIDI page reads them ([`lengthen_steps`]); written as they are, a sampler
/// would barely sound them.
pub fn write_clip(clip: &MidiClip) -> Result<Written, WriteError> {
    let mut song = SmfSong::from_clip(clip);
    let sixteenth = i64::from(clip.ppq / 4);
    let longest = |_: u8| sixteenth;
    for track in &mut song.tracks {
        let notes = std::mem::take(&mut track.notes);
        track.notes = lengthen_steps(notes, clip.end.unwrap_or(i64::MAX), Some(&longest));
    }
    write(&song, &WriteOptions::default())
}

/// Gives steps from FL's step sequencer, which have no length, the length FL plays them for: FL plays a step until its
/// sound ends. Here that's until the next note on the same key or the end of the clip, and no longer than `longest`
/// gives for the key (how long its sample plays), when it knows. Other notes stay as they are.
pub fn lengthen_steps(notes: Vec<NoteEvent>, clip_length: i64, longest: Option<&dyn Fn(u8) -> i64>) -> Vec<NoteEvent> {
    if !notes.iter().any(|n| n.step) {
        return notes;
    }
    // Each key's starts in order, so the next is found by halves: a whole song's steps can all be on one key.
    // A muted note plays nothing, so it doesn't cut a step short.
    let mut starts: HashMap<u8, Vec<i64>> = HashMap::new();
    for n in notes.iter().filter(|n| !n.muted) {
        starts.entry(n.key).or_default().push(n.start);
    }
    for on in starts.values_mut() {
        on.sort_unstable();
        on.dedup();
    }
    notes
        .into_iter()
        .map(|n| {
            if !n.step {
                return n;
            }
            let on = starts.get(&n.key).map(Vec::as_slice).unwrap_or_default();
            let next = on.partition_point(|&s| s <= n.start);
            let next = on.get(next).copied().unwrap_or(clip_length);
            let mut length = next.min(clip_length).saturating_sub(n.start);
            if let Some(most) = longest {
                length = length.min(most(n.key));
            }
            NoteEvent { length: n.length.max(length), ..n }
        })
        .collect()
}

/// A song as the bytes of a Standard MIDI File, with what the file couldn't keep.
pub fn write(song: &SmfSong, options: &WriteOptions) -> Result<Written, WriteError> {
    if song.ppq == 0 || song.ppq > 0x7FFF {
        return Err(WriteError::BadPpq(song.ppq));
    }
    let single = match options.format {
        SmfFormat::Zero if song.tracks.len() > 1 => {
            return Err(WriteError::TooManyTracksForFormat0(song.tracks.len()));
        }
        SmfFormat::Zero => true,
        SmfFormat::One => false,
        SmfFormat::Auto => song.tracks.len() <= 1,
    };

    // The song's own events, at its start: its name, tempo and time signature.
    let tempo = match song.tempo {
        Some(bpm) if options.tempo => Some(tempo_event(bpm)?),
        _ => None,
    };
    let time_signature = song.time_signature.map(time_signature_event).transpose()?;
    let song_events = |name: &str| {
        let mut events = vec![text_event(0x03, name)];
        events.extend(tempo.clone());
        events.extend(time_signature.clone());
        events
    };

    let mut tracks = Vec::new();
    let mut dropped = 0;
    if single {
        let track = song.tracks.first();
        let name = match track {
            Some(t) if !t.name.is_empty() => &t.name,
            _ => &song.name,
        };
        let empty = SmfTrack::default();
        tracks.push(track_chunk(0, song_events(name), track.unwrap_or(&empty), song.end, options.tidy, &mut dropped)?);
    } else {
        // The first track holds no notes: the song's name and timing only, ending at once. It's no track of the
        // song's, and holds nothing that can go wrong, so the 0 it's given never shows in an error.
        tracks.push(track_chunk(0, song_events(&song.name), &SmfTrack::default(), None, options.tidy, &mut dropped)?);
        for (i, track) in song.tracks.iter().enumerate() {
            let name = vec![text_event(0x03, &track.name)];
            tracks.push(track_chunk(i, name, track, song.end, options.tidy, &mut dropped)?);
        }
    }

    let count = u16::try_from(tracks.len()).map_err(|_| WriteError::TooManyTracks(tracks.len()))?;
    let format: u16 = if single { 0 } else { 1 };
    let mut out = Vec::with_capacity(14 + tracks.iter().map(Vec::len).sum::<usize>());
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&format.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&song.ppq.to_be_bytes());
    for t in tracks {
        out.extend(t);
    }
    Ok(Written { bytes: out, dropped_controllers: dropped, shared_channels: shared_channels(&song.tracks) })
}

/// The tracks (by place in [`SmfSong::tracks`]) on a channel an earlier track uses with another sound (a track that
/// sets none counts as one more sound: the instrument's own). A channel plays one sound at a time, so these play with
/// the earlier track's sound, and a reader gives them its sound too.
fn shared_channels(tracks: &[SmfTrack]) -> Vec<usize> {
    // Each channel's first track's sound, once a track is on it.
    let mut sounds: [Option<Option<u8>>; CHANNELS as usize] = [None; CHANNELS as usize];
    let mut clashes = Vec::new();
    for (i, t) in tracks.iter().enumerate() {
        let Some(slot) = sounds.get_mut(usize::from(t.channel)) else { continue };
        match slot {
            Some(earlier) if *earlier != t.program => clashes.push(i),
            Some(_) => {}
            None => *slot = Some(t.program),
        }
    }
    clashes
}

/// A channel message at a tick. `rank` puts messages at the same tick in order: the sound first, then notes ending,
/// then controllers, then notes starting (so a note starts with its controllers already set), then key pressure (on
/// the notes just started). Messages of the same rank keep the order they were added in.
struct Message {
    tick: i64,
    rank: u8,
    bytes: [u8; 3],
    len: u8,
}

const RANK_PROGRAM: u8 = 0;
const RANK_NOTE_OFF: u8 = 1;
const RANK_CONTROLLER: u8 = 2;
const RANK_NOTE_ON: u8 = 3;
/// Key pressure presses a note that's sounding, so at the tick a note starts it comes after the note-on.
const RANK_KEY_PRESSURE: u8 = 4;

/// One `MTrk` chunk: the meta events at its start, then the track's sound, notes and controllers, then its end.
fn track_chunk(
    index: usize,
    metas: Vec<Vec<u8>>,
    track: &SmfTrack,
    end: Option<i64>,
    tidy: Tidy,
    dropped: &mut usize,
) -> Result<Vec<u8>, WriteError> {
    let ch = track.channel;
    if ch >= CHANNELS {
        return Err(WriteError::ChannelOutOfRange { track: index, channel: ch });
    }
    let mut messages = Vec::new();
    let mut push = |tick: i64, rank: u8, bytes: &[u8]| {
        let mut b = [0u8; 3];
        for (slot, &v) in b.iter_mut().zip(bytes) {
            *slot = v;
        }
        messages.push(Message { tick, rank, bytes: b, len: bytes.len() as u8 });
    };

    if let Some(program) = track.program {
        if program > 127 {
            return Err(WriteError::ProgramOutOfRange { track: index, program });
        }
        push(0, RANK_PROGRAM, &[0xC0 | ch, program]);
    }
    for note in tidy_notes(index, &track.notes, tidy)? {
        push(note.start, RANK_NOTE_ON, &[0x90 | ch, note.key, note.velocity]);
        // Release velocity 0, as the C# writer writes it (the model keeps no release velocity).
        push(note.end, RANK_NOTE_OFF, &[0x80 | ch, note.key, 0]);
    }
    for c in &track.controllers {
        if c.tick < 0 {
            return Err(WriteError::NegativeTime { track: index, tick: c.tick });
        }
        let out_of_range = Err(WriteError::ControllerOutOfRange { track: index, kind: c.kind });
        let seven = controller_to_7(c.value);
        let fourteen = controller_to_14(c.value);
        let (msb, lsb) = ((fourteen >> 7) as u8, (fourteen & 0x7F) as u8);
        match c.kind {
            ControllerKind::Control(n) if n > 127 => return out_of_range,
            ControllerKind::Control(n) => push(c.tick, RANK_CONTROLLER, &[0xB0 | ch, n, seven]),
            ControllerKind::PitchBend => push(c.tick, RANK_CONTROLLER, &[0xE0 | ch, lsb, msb]),
            ControllerKind::ChannelPressure => push(c.tick, RANK_CONTROLLER, &[0xD0 | ch, seven]),
            ControllerKind::KeyPressure(k) if k > 127 => return out_of_range,
            ControllerKind::KeyPressure(k) => push(c.tick, RANK_KEY_PRESSURE, &[0xA0 | ch, k, seven]),
            // MIDI 1.0's RPN and NRPN: choose the controller (bank, then index), then its value's 14 bits as data
            // entry (6, then 38), as MIDI 2.0's translation to MIDI 1.0 does.
            ControllerKind::Registered(bank, i) | ControllerKind::Assignable(bank, i) => {
                if bank > 127 || i > 127 {
                    return out_of_range;
                }
                let (bank_cc, index_cc) =
                    if matches!(c.kind, ControllerKind::Registered(..)) { (101, 100) } else { (99, 98) };
                for (cc, v) in [(bank_cc, bank), (index_cc, i), (6, msb), (38, lsb)] {
                    push(c.tick, RANK_CONTROLLER, &[0xB0 | ch, cc, v]);
                }
            }
            // Per-note and relative controllers are MIDI 2.0's own: MIDI 1.0 has no message for them. Counted, so the
            // caller can say so.
            _ => *dropped = dropped.saturating_add(1),
        }
    }
    // Stable, so messages at the same tick and rank keep their order (an RPN's four controllers, say).
    messages.sort_by_key(|m| (m.tick, m.rank));

    let mut body = Vec::new();
    for meta in metas {
        body.push(0x00);
        body.extend(meta);
    }
    let mut last = 0i64;
    for m in &messages {
        push_delta(&mut body, index, last, m.tick)?;
        body.extend(m.bytes.iter().take(usize::from(m.len)));
        last = m.tick;
    }
    // The track ends at the song's end, if that's after its last event.
    push_delta(&mut body, index, last, end.unwrap_or(0).max(last))?;
    body.extend_from_slice(&[0xFF, 0x2F, 0x00]);

    let length = u32::try_from(body.len()).map_err(|_| WriteError::TrackTooLong { track: index })?;
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&length.to_be_bytes());
    out.extend(body);
    Ok(out)
}

/// A note ready to write: where it starts and ends, its key and its MIDI 1.0 velocity.
struct Tidied {
    start: i64,
    end: i64,
    key: u8,
    velocity: u8,
}

/// The track's notes checked, and fixed (or refused) where a MIDI 1.0 file can't hold them: see [`Tidy`].
fn tidy_notes(track: usize, notes: &[NoteEvent], tidy: Tidy) -> Result<Vec<Tidied>, WriteError> {
    let mut out = Vec::with_capacity(notes.len());
    for n in notes {
        if n.key > 127 {
            return Err(WriteError::KeyOutOfRange { track, key: n.key });
        }
        if n.start < 0 {
            return Err(WriteError::NegativeTime { track, tick: n.start });
        }
        if n.length < 0 {
            return Err(WriteError::NegativeLength { track, start: n.start, length: n.length });
        }
        let length = match (n.length, tidy) {
            (0, Tidy::Refuse) => return Err(WriteError::ZeroLength { track, start: n.start, key: n.key }),
            (0, Tidy::Fix) => 1,
            (l, _) => l,
        };
        let end = n.start.checked_add(length).ok_or(WriteError::TooFar { track, tick: n.start })?;
        out.push(Tidied { start: n.start, end, key: n.key, velocity: n.velocity7() });
    }
    // Each key's notes in order of start (stable, so notes starting together keep their order).
    out.sort_by_key(|n| (n.key, n.start));
    let mut kept: Vec<Tidied> = Vec::with_capacity(out.len());
    for n in out {
        match kept.last_mut() {
            Some(prev) if prev.key == n.key && prev.end > n.start => {
                if tidy == Tidy::Refuse {
                    return Err(WriteError::Overlap { track, start: n.start, key: n.key });
                }
                if prev.start == n.start {
                    // Two notes struck together on one key sound as one: the longer and louder.
                    prev.end = prev.end.max(n.end);
                    prev.velocity = prev.velocity.max(n.velocity);
                    continue;
                }
                // The earlier note ends where the key starts again.
                prev.end = n.start;
                kept.push(n);
            }
            _ => kept.push(n),
        }
    }
    Ok(kept)
}

/// The gap from `last` to `tick` as a variable-length number ([`push_vlq`]).
fn push_delta(out: &mut Vec<u8>, track: usize, last: i64, tick: i64) -> Result<(), WriteError> {
    let delta = tick.checked_sub(last).filter(|d| (0..=MAX_DELTA).contains(d));
    let Some(delta) = delta.and_then(|d| u32::try_from(d).ok()) else {
        return Err(WriteError::TooFar { track, tick });
    };
    push_vlq(out, delta);
    Ok(())
}

/// A variable-length number, as a Standard MIDI File writes gaps between events and the lengths of meta events:
/// seven bits a byte, highest first, every byte but the last with its top bit set. At most 0x0FFF_FFFF (four bytes);
/// callers keep to that.
fn push_vlq(out: &mut Vec<u8>, value: u32) {
    let mut started = false;
    for shift in [21u32, 14, 7] {
        let part = ((value >> shift) & 0x7F) as u8;
        if started || part != 0 {
            out.push(part | 0x80);
            started = true;
        }
    }
    out.push((value & 0x7F) as u8);
}

/// A text meta event (type 3 is a track's name), without its time: UTF-8, cut to 255 bytes at a whole letter.
fn text_event(kind: u8, text: &str) -> Vec<u8> {
    let mut cut = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let bytes = text.as_bytes().get(..cut).unwrap_or_default();
    // The length is a variable-length number like any other: 128 bytes and up take two bytes to say.
    let mut out = vec![0xFF, kind];
    push_vlq(&mut out, cut as u32);
    out.extend_from_slice(bytes);
    out
}

/// The tempo meta event: microseconds a beat, in three bytes.
fn tempo_event(bpm: f64) -> Result<Vec<u8>, WriteError> {
    let us = 60_000_000.0 / bpm;
    if !bpm.is_finite() || bpm <= 0.0 || !(1.0..=f64::from(0xFF_FFFFu32)).contains(&us.round()) {
        return Err(WriteError::BadTempo(bpm));
    }
    let [_, a, b, c] = (us.round() as u32).to_be_bytes();
    Ok(vec![0xFF, 0x51, 0x03, a, b, c])
}

/// The time signature meta event: the beats a bar, the beat as a power of two, then MIDI's usual 24 clocks a click
/// and 8 thirty-second notes a beat.
fn time_signature_event(t: TimeSignature) -> Result<Vec<u8>, WriteError> {
    if t.numerator == 0 || !t.denominator.is_power_of_two() {
        return Err(WriteError::BadTimeSignature(t));
    }
    Ok(vec![0xFF, 0x58, 0x04, t.numerator, t.denominator.trailing_zeros() as u8, 24, 8])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale_up;

    #[test]
    fn variable_length_numbers_are_written_as_the_smf_spec_shows() {
        // The examples in the Standard MIDI File spec.
        for (value, bytes) in [
            (0x00, vec![0x00]),
            (0x7F, vec![0x7F]),
            (0x80, vec![0x81, 0x00]),
            (0x2000, vec![0xC0, 0x00]),
            (0x3FFF, vec![0xFF, 0x7F]),
            (0x4000, vec![0x81, 0x80, 0x00]),
            (0x0FFF_FFFF, vec![0xFF, 0xFF, 0xFF, 0x7F]),
        ] {
            let mut out = Vec::new();
            push_delta(&mut out, 0, 0, value).unwrap();
            assert_eq!(out, bytes, "{value:#x}");
        }
        assert_eq!(
            push_delta(&mut Vec::new(), 0, 0, 0x1000_0000),
            Err(WriteError::TooFar { track: 0, tick: 0x1000_0000 })
        );
    }

    #[test]
    fn controllers_come_back_from_32_bits_as_they_were() {
        for v in 0..=127u32 {
            assert_eq!(u32::from(controller_to_7(scale_up(v, 7, 32))), v);
        }
        for v in 0..=0x3FFFu32 {
            assert_eq!(u32::from(controller_to_14(scale_up(v, 14, 32))), v);
        }
        assert_eq!(
            (controller_to_14(0x8000_0000), controller_to_7(u32::MAX), controller_to_14(u32::MAX)),
            (8192, 127, 0x3FFF)
        );
    }

    #[test]
    fn long_names_are_cut_at_a_whole_letter() {
        let name = "é".repeat(200);
        let event = text_event(3, &name);
        // 254 bytes (127 two-byte letters), said as a variable-length number: 0x81 0x7E.
        assert_eq!(&event[..4], [0xFF, 3, 0x81, 0x7E]);
        assert_eq!(event.len(), 4 + 254);
        assert!(std::str::from_utf8(&event[4..]).is_ok());
        // Up to 127 bytes the length is one byte.
        assert_eq!(&text_event(3, &"a".repeat(127))[..3], [0xFF, 3, 0x7F]);
        assert_eq!(&text_event(3, &"a".repeat(128))[..4], [0xFF, 3, 0x81, 0x00]);
    }
}
