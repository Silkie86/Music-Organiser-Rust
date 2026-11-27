//! Writes a new FL Studio 25 project from a song's arrangement (any DAW's, as the readers give it): its tempo, time
//! signature, markers, tracks and their names, a pattern for each clip of notes, and its audio clips. What can't go
//! across (plugins and their settings, automation, a stretched clip's stretch) is listed in [`Written::left_out`].
//!
//! The project is built on FL Studio 25's own empty project (the recipe test save "MO Recipe 01 empty", FL 25.2.5,
//! with its registration text, project folder and dates taken out as it's read) and FL's own audio clip channel (from
//! recipe save 04, without its sample), so every event FL expects is there, in the order FL writes it. What's added
//! follows the recipe saves:
//!
//! - Each instrument track becomes an empty Sampler channel named after the track (FL can't load another DAW's
//!   instrument); each audio file an audio clip channel (type 4) with its path (event 196).
//! - Patterns come before the channels: 65 starts one, 193 names it, 224 holds its notes as 24-byte records (u32
//!   position, u16 flags 0x4000, u16 channel, u32 length, u16 key, fine pitch 120 at 16, release 64 at 18, 0x20 at 19
//!   when muted, pan 64 at 20, velocity 0 to 128 at 21, 128 and 128 at 22 and 23).
//! - The playlist (233) holds 80-byte items: u32 position, u16 20480, u16 the pattern (20480 + its number) or the
//!   audio channel, u32 length, u16 the track (499 for track 1, counting down), u16 flags at 18 (0x40, and 0x2000 when
//!   muted), at 24 and 28 a pattern's -1 or an audio clip's start and end in the file in milliseconds (-1.0 for the
//!   whole file), a number for each item at 32, fades in milliseconds at 36 and 44, 1.0 as a float at 52 on an audio
//!   clip, 3 at 56 when it has fades, and 1.0 as a double at 64.
//! - A marker is 148 (its position), 33 and 34 (4 and 4), 46, 45, 168, 101 (0), then 205 its name, after the playlist.
//! - A track's name (239) follows its record (238), whose byte 12 is 0 when the track is switched off.
//!
//! The project is at 96 ticks a beat, as FL's empty project is.

use std::collections::HashMap;

use super::events::{
    self, EV_CHANNEL_NEW, EV_PATTERN_NAME, EV_PATTERN_NEW, EV_PATTERN_NOTES, EV_PLAYLIST, EV_PLUGIN_NAME,
    EV_SAMPLE_PATH, EV_TEMPO, EV_TITLE, Event,
};
use crate::arrangement::{Arrangement, AudioPart, LaneKind, PreviewClip, PreviewNote};

/// FL Studio 25's empty project (recipe save 01).
const TEMPLATE: &[u8] = include_bytes!("../../tests/fixtures/recipe/fl/MO Recipe 01 empty.flp");
/// A project with FL Studio 25's audio clip channel as its third channel (recipe save 04).
const AUDIO_TEMPLATE: &[u8] = include_bytes!("../../tests/fixtures/recipe/fl/MO Recipe 04 audio clip.flp");
/// The empty project's events that are about whoever saved it: registration text, project folder, dates.
const NOT_TAKEN: [u8; 3] = [200, 202, EV_PROJECT_TIME];
/// The patterns FL can hold.
const PATTERNS: usize = 999;

/// Ticks a beat in the written project (the empty project's own).
pub const PPQ: f64 = 96.0;
/// The tracks FL 25's playlist has.
const TRACKS: usize = 500;
/// FL Studio's slowest and fastest tempo.
const SLOWEST: f64 = 10.0;
const FASTEST: f64 = 522.0;
/// A pattern clip's item number base: an item above it is a pattern, at or below it a channel.
const PATTERN_BASE: u16 = 20480;

const EV_TIME_SIG_NUMERATOR: u8 = 17;
const EV_TIME_SIG_DENOMINATOR: u8 = 18;
/// The event after the patterns in FL's empty project: the patterns go just before it.
const AFTER_PATTERNS: u8 = 226;
const EV_ARRANGEMENT_NEW: u8 = 99;
const EV_TRACK_DATA: u8 = 238;
const EV_TRACK_NAME: u8 = 239;
const EV_PROJECT_TIME: u8 = 237;
/// The event FL writes the project's dates (237) after.
const BEFORE_PROJECT_TIME: u8 = 195;
const EV_TIME_MARKER: u8 = 148;
const EV_MARKER_NAME: u8 = 205;
/// The event FL writes just after an audio clip channel's sample path (196).
const AFTER_SAMPLE_PATH: u8 = 170;
/// The block of settings FL writes after a channel's plugin type, before its name.
const BEFORE_CHANNEL_NAME: u8 = 212;

/// A project written, and what of the song couldn't go into it.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub bytes: Vec<u8>,
    pub left_out: Vec<String>,
}

/// A channel to write: an empty Sampler for a track's notes, or an audio clip channel for a file.
#[derive(Clone, Debug, PartialEq)]
enum Channel {
    Sampler(String),
    Audio(String),
}

/// A pattern: its name and notes (each note record names its channel; positions in ticks from its start).
#[derive(Clone, Debug, PartialEq)]
struct Pattern {
    name: String,
    notes: Vec<[u8; 24]>,
}

/// One playlist item before it's laid out: where, how long, what (pattern number above the base, or channel), on
/// which track, muted, the audio's start and end in milliseconds and its fades.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    at: u32,
    length: u32,
    what: u16,
    track: u16,
    muted: bool,
    audio: Option<(f32, f32, f32, f32)>,
}

/// The song as a new FL Studio project. `created` is when it's made, as FL's own date (days since 30 Dec 1899).
pub fn write(song: &Arrangement, title: &str, created: f64) -> Written {
    let mut left_out = Vec::new();
    let tempo = tempo_of(song);
    if (tempo - song.tempo).abs() > 1e-9 {
        left_out.push(format!("The tempo ({:.0} BPM): FL's is {SLOWEST:.0} to {FASTEST:.0}.", song.tempo));
    }
    let mut channels: Vec<Channel> = Vec::new();
    let mut patterns: Vec<Pattern> = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    let mut track_names: Vec<(String, bool)> = Vec::new();
    let mut audio_channel: HashMap<String, u16> = HashMap::new();
    let mut same_pattern: HashMap<(u16, Vec<[u8; 24]>, u32), u16> = HashMap::new();
    let mut empty_pattern: HashMap<(String, u32), u16> = HashMap::new();
    let mut stretched = false;
    let mut automation = false;
    let mut too_many_patterns = false;
    let mut gain = false;
    let mut early_notes = 0usize;
    // A pattern is whole bars of the song's time signature.
    let bar = ticks(song.signature.beats()).unwrap_or(4 * PPQ as u32).max(1);

    for lane in &song.lanes {
        match lane.kind {
            LaneKind::Instrument | LaneKind::Audio => {}
            LaneKind::Automation => {
                automation |= !lane.clips.is_empty();
                continue;
            }
            LaneKind::Group => continue,
        }
        if track_names.len() >= TRACKS {
            left_out.push(format!("Tracks after the first {TRACKS} (FL's playlist has no more)."));
            break;
        }
        let track = (TRACKS - 1 - track_names.len()) as u16;
        track_names.push((lane.name.clone(), lane.muted));
        let mut sampler: Option<u16> = None;
        for clip in &lane.clips {
            // A clip that starts before the song does (a Studio One part a 20th of a beat early, seen on Solomon's
            // songs) starts with the song instead: FL's playlist has nothing before its start.
            let from_start;
            let clip = if clip.start < 0.0 && clip.start + clip.length > 0.0 {
                let (moved, early) = from_song_start(clip, 60.0 / tempo);
                early_notes += early;
                from_start = moved;
                &from_start
            } else {
                clip
            };
            let Some(at) = ticks(clip.start) else { continue };
            if !clip.notes.is_empty() {
                let channel = *sampler.get_or_insert_with(|| {
                    channels.push(Channel::Sampler(lane.name.clone()));
                    (channels.len() - 1) as u16
                });
                let (notes, length) = pattern_notes(clip, channel, bar);
                let key = (channel, notes, length);
                let number = match same_pattern.get(&key) {
                    Some(n) => *n,
                    None if patterns.len() >= PATTERNS => {
                        too_many_patterns = true;
                        continue;
                    }
                    None => {
                        let name = if clip.name.trim().is_empty() { lane.name.clone() } else { clip.name.clone() };
                        patterns.push(Pattern { name, notes: key.1.clone() });
                        let n = patterns.len() as u16;
                        same_pattern.insert(key.clone(), n);
                        n
                    }
                };
                // FL repeats a pattern clip longer than its pattern: the clip stops where its notes' bars do.
                let length = clip_ticks(clip).min(key.2);
                items.push(Item { at, length, what: PATTERN_BASE + number, track, muted: clip.muted, audio: None });
            } else if clip.audio.is_empty() && clip.points.is_empty() {
                // A clip with nothing in it (an empty pattern laid out as a section): an empty pattern of its name,
                // as long as the clip's whole bars.
                let name = if clip.name.trim().is_empty() { lane.name.clone() } else { clip.name.clone() };
                let length = ticks(clip.length).unwrap_or(0).max(1);
                let bars = length.div_ceil(bar).saturating_mul(bar);
                let number = match empty_pattern.get(&(name.clone(), bars)) {
                    Some(n) => *n,
                    None if patterns.len() >= PATTERNS => {
                        too_many_patterns = true;
                        continue;
                    }
                    None => {
                        patterns.push(Pattern { name: name.clone(), notes: Vec::new() });
                        let n = patterns.len() as u16;
                        empty_pattern.insert((name, bars), n);
                        n
                    }
                };
                items.push(Item { at, length, what: PATTERN_BASE + number, track, muted: clip.muted, audio: None });
            }
            for part in &clip.audio {
                let channel = *audio_channel.entry(part.path.clone()).or_insert_with(|| {
                    channels.push(Channel::Audio(part.path.clone()));
                    (channels.len() - 1) as u16
                });
                let Some(part_at) = ticks(clip.start + part.start) else { continue };
                let rate = part.seconds_per_beat.unwrap_or(60.0 / tempo);
                if (rate - 60.0 / tempo).abs() > 1e-6 || part.semitones != 0.0 || part.reversed {
                    stretched = true;
                }
                gain |= part.gain_db != 0.0;
                let from = (part.from * 1000.0) as f32;
                let to = ((part.from + part.length * rate) * 1000.0) as f32;
                let (fade_in, fade_out) = clip.fades_of(part);
                let ms = |beats: f64| (beats * 60_000.0 / tempo) as f32;
                let (fade_in, fade_out) = (ms(fade_in), ms(fade_out));
                items.push(Item {
                    at: part_at,
                    length: ticks(part.length).unwrap_or(0).max(1),
                    what: channel,
                    track,
                    muted: clip.muted,
                    audio: Some((from, to, fade_in, fade_out)),
                });
            }
        }
    }
    if stretched {
        left_out.push(
            "Audio that's stretched, transposed or reversed plays at its own speed and pitch: FL stretches it in the \
             channel's settings."
                .into(),
        );
    }
    if gain {
        left_out.push("Audio clips' own volume: they play at full volume.".into());
    }
    if early_notes > 0 {
        left_out.push(format!(
            "Where {} before the song's start play{}: on its very start.",
            if early_notes == 1 { "a note".to_owned() } else { format!("{early_notes} notes") },
            if early_notes == 1 { "s" } else { "" }
        ));
    }
    if automation {
        left_out.push("Automation.".into());
    }
    if too_many_patterns {
        left_out.push(format!("Clips after the first {PATTERNS} different ones (FL holds no more patterns)."));
    }
    if channels.iter().any(|c| matches!(c, Channel::Sampler(_))) {
        left_out.push("Instruments: each instrument track gets an empty Sampler named after it.".into());
    }
    if !track_names.is_empty() {
        left_out.push("Effects and mixer settings.".into());
    }
    for note in &song.notes {
        left_out.push(note.clone());
    }
    items.sort_by_key(|i| (i.at, std::cmp::Reverse(i.track)));
    let bytes = assemble(song, title, created, &channels, &patterns, &items, &track_names);
    Written { bytes, left_out }
}

/// The tempo the copy is written at: the song's, inside FL's range (120 when it isn't a number).
fn tempo_of(song: &Arrangement) -> f64 {
    if song.tempo.is_finite() { song.tempo.clamp(SLOWEST, FASTEST) } else { 120.0 }
}

/// A clip that starts before the song, from the song's start: its notes where they were (one before the start on the
/// start), its audio cut at the start. Also how many notes were moved to the start. `spb` is the song's seconds a beat,
/// for audio that doesn't say its own.
fn from_song_start(clip: &PreviewClip, spb: f64) -> (PreviewClip, usize) {
    let shift = clip.start;
    let mut early = 0;
    let notes = clip
        .notes
        .iter()
        .map(|n| {
            let start = n.start + shift;
            if start < 0.0 {
                early += 1;
            }
            PreviewNote { start: start.max(0.0), ..*n }
        })
        .collect();
    let audio = clip
        .audio
        .iter()
        .filter_map(|p| {
            let start = p.start + shift;
            if start >= 0.0 {
                return Some(AudioPart { start, ..p.clone() });
            }
            let cut = -start;
            if p.length <= cut {
                return None;
            }
            // A reversed part plays from its end back to `from`, so cutting its start leaves `from` where it is.
            let from = if p.reversed { p.from } else { p.from + cut * p.seconds_per_beat.unwrap_or(spb) };
            Some(AudioPart { start: 0.0, length: p.length - cut, from, ..p.clone() })
        })
        .collect();
    let moved = PreviewClip {
        name: clip.name.clone(),
        start: 0.0,
        length: clip.length + shift,
        muted: clip.muted,
        notes,
        audio,
        points: Vec::new(),
        colour: clip.colour,
        fade_in: (clip.fade_in + shift).max(0.0),
        fade_out: clip.fade_out.min(clip.length + shift),
        parts: clip.parts.clone(),
    };
    (moved, early)
}

/// Beats in the written project's ticks, when they're a place in the song (not before its start or past FL's end).
fn ticks(beats: f64) -> Option<u32> {
    let t = (beats * PPQ).round();
    (t.is_finite() && (0.0..u32::MAX as f64).contains(&t)).then_some(t as u32)
}

/// How long a clip of notes is in the written project's ticks: at least one, so its notes have a tick to be on, and
/// all of FL's ticks for one longer than they reach (a damaged save's clip can be 1e20 beats long).
fn clip_ticks(clip: &PreviewClip) -> u32 {
    match ticks(clip.length) {
        Some(t) => t.max(1),
        None if clip.length > 0.0 => u32::MAX,
        None => 1,
    }
}

/// A clip's notes as a pattern's records, and how long the pattern is: its notes' last bar.
///
/// A note the reader gives in the clip stays in it here: one that rounds onto the clip's end tick (a note 0.00003 of
/// a beat before a Bitwig clip's end) starts on the clip's last tick instead, and one that rounds to a tick before its
/// start (a hair before it) on its first. FL doesn't play a note at or after the end of its playlist item, so without
/// this such a note was lost and the copy refused to save (the real-song test of 27 Nov 2026: 87 of Solomon's Bitwig
/// and Studio One saves). Only rounding is put right: a note further out is left out, so the copy's check sees it.
fn pattern_notes(clip: &PreviewClip, channel: u16, bar: u32) -> (Vec<[u8; 24]>, u32) {
    let mut records = Vec::new();
    let mut end = 0u32;
    let last_tick = clip_ticks(clip).saturating_sub(1);
    for n in &clip.notes {
        let at = (n.start * PPQ).round();
        // Within a tick of the clip's edges, rounding moved it there; further out, the note isn't the clip's.
        if !(-1.0..=f64::from(last_tick) + 1.0).contains(&at) {
            continue;
        }
        let at = at.clamp(0.0, f64::from(last_tick)) as u32;
        let Some(length) = ticks(n.length) else { continue };
        let length = length.max(1);
        end = end.max(at.saturating_add(length));
        let mut r = [0u8; 24];
        put(&mut r, 0, &at.to_le_bytes());
        put(&mut r, 4, &0x4000u16.to_le_bytes());
        put(&mut r, 6, &channel.to_le_bytes());
        put(&mut r, 8, &length.to_le_bytes());
        put(&mut r, 12, &u16::from(n.key).to_le_bytes());
        let velocity = (f64::from(n.velocity) / f64::from(u16::MAX) * 128.0).round().clamp(0.0, 128.0) as u8;
        put(&mut r, 16, &[120, 0, 64, if n.muted { 0x20 } else { 0 }, 64, velocity, 128, 128]);
        records.push(r);
    }
    records.sort_by_key(|r| (r.first_chunk::<4>().map(|b| u32::from_le_bytes(*b)), r.get(12).copied()));
    let length = end.div_ceil(bar).max(1).saturating_mul(bar);
    (records, length)
}

/// Puts `bytes` into a record at `at` (the records are fixed-size, so it always fits).
fn put(record: &mut [u8], at: usize, bytes: &[u8]) {
    if let Some(slot) = record.get_mut(at..at + bytes.len()) {
        slot.copy_from_slice(bytes);
    }
}

/// FL's text: UTF-16, ending in a zero.
fn text(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// An event's bytes as FL writes them: its id, then one, two or four bytes, or a length and the data.
fn encode(id: u8, data: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    if id < 192 {
        // A fixed-size event (or FL 25's extended event 172, whose data says its own size) is written as it is.
        out.extend_from_slice(data);
        return;
    }
    let mut n = data.len();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(data);
}

fn assemble(
    song: &Arrangement,
    title: &str,
    created: f64,
    channels: &[Channel],
    patterns: &[Pattern],
    items: &[Item],
    track_names: &[(String, bool)],
) -> Vec<u8> {
    let Ok(template) = events::load(TEMPLATE) else { return Vec::new() };
    let all: Vec<&Event<'_>> = template.events.iter().filter(|e| !NOT_TAKEN.contains(&e.id)).collect();
    let sampler_block: Vec<&Event<'_>> = {
        let start = all.iter().position(|e| e.id == EV_CHANNEL_NEW);
        let end = all.iter().position(|e| e.id == EV_ARRANGEMENT_NEW);
        match (start, end) {
            (Some(s), Some(e)) if s < e => all.get(s..e).unwrap_or_default().to_vec(),
            _ => Vec::new(),
        }
    };
    let Ok(audio_template) = events::load(AUDIO_TEMPLATE) else { return Vec::new() };
    let audio_block = audio_channel_block(&audio_template.events);
    let mut body = Vec::new();
    let mut patterns_written = false;
    let mut channels_written = false;
    let mut in_first_arrangement = false;
    let mut arrangements = 0;
    let write_patterns = |body: &mut Vec<u8>| {
        for (i, p) in patterns.iter().enumerate() {
            encode(EV_PATTERN_NEW, &((i + 1) as u16).to_le_bytes(), body);
            encode(EV_PATTERN_NAME, &text(&p.name), body);
            // FL writes no notes event for a pattern without notes.
            if !p.notes.is_empty() {
                encode(EV_PATTERN_NOTES, &p.notes.concat(), body);
            }
        }
    };
    let mut skipping_channel = false;
    for e in all {
        let data: &[u8] = &e.data;
        if skipping_channel {
            if e.id == EV_ARRANGEMENT_NEW {
                skipping_channel = false;
            } else {
                continue;
            }
        }
        match e.id {
            EV_TEMPO => encode(EV_TEMPO, &((tempo_of(song) * 1000.0).round() as u32).to_le_bytes(), &mut body),
            EV_TIME_SIG_NUMERATOR => encode(e.id, &[song.signature.top().min(255) as u8], &mut body),
            EV_TIME_SIG_DENOMINATOR => encode(e.id, &[song.signature.bottom().min(255) as u8], &mut body),
            EV_TITLE => encode(EV_TITLE, &text(title), &mut body),
            BEFORE_PROJECT_TIME => {
                encode(e.id, data, &mut body);
                let mut time = created.to_le_bytes().to_vec();
                time.extend_from_slice(&0f64.to_le_bytes());
                encode(EV_PROJECT_TIME, &time, &mut body);
            }
            AFTER_PATTERNS if !patterns_written => {
                patterns_written = true;
                write_patterns(&mut body);
                encode(e.id, data, &mut body);
            }
            EV_CHANNEL_NEW if !channels_written => {
                channels_written = true;
                skipping_channel = true;
                if !patterns_written {
                    patterns_written = true;
                    write_patterns(&mut body);
                }
                if channels.is_empty() {
                    // The empty project's own Sampler, as FL makes every project with one.
                    for c in &sampler_block {
                        encode(c.id, &c.data, &mut body);
                    }
                }
                for (i, c) in channels.iter().enumerate() {
                    let number = (i as u16).to_le_bytes();
                    match c {
                        Channel::Sampler(name) => {
                            for t in &sampler_block {
                                match t.id {
                                    EV_CHANNEL_NEW => encode(t.id, &number, &mut body),
                                    EV_PLUGIN_NAME => encode(t.id, &text(name), &mut body),
                                    _ => encode(t.id, &t.data, &mut body),
                                }
                            }
                        }
                        Channel::Audio(path) => {
                            let name = crate::paths::stem(crate::paths::file_name(path));
                            for t in &audio_block {
                                match t.id {
                                    EV_CHANNEL_NEW => encode(t.id, &number, &mut body),
                                    AFTER_SAMPLE_PATH => {
                                        encode(EV_SAMPLE_PATH, &text(path), &mut body);
                                        encode(t.id, &t.data, &mut body);
                                    }
                                    _ => encode(t.id, &t.data, &mut body),
                                }
                                // The channel's name follows the block of settings that starts it, as FL writes it.
                                if t.id == BEFORE_CHANNEL_NAME {
                                    encode(EV_PLUGIN_NAME, &text(name), &mut body);
                                }
                            }
                        }
                    }
                }
            }
            EV_ARRANGEMENT_NEW => {
                arrangements += 1;
                in_first_arrangement = arrangements == 1;
                encode(e.id, data, &mut body);
            }
            EV_PLAYLIST if in_first_arrangement => {
                encode(EV_PLAYLIST, &playlist(items), &mut body);
                for m in &song.markers {
                    let Some(at) = ticks(m.beat) else { continue };
                    encode(EV_TIME_MARKER, &at.to_le_bytes(), &mut body);
                    encode(33, &[4], &mut body);
                    encode(34, &[4], &mut body);
                    encode(46, &[0], &mut body);
                    encode(45, &[0], &mut body);
                    encode(168, &0u32.to_le_bytes(), &mut body);
                    encode(101, &0u16.to_le_bytes(), &mut body);
                    encode(EV_MARKER_NAME, &text(&m.name), &mut body);
                }
            }
            EV_TRACK_DATA if in_first_arrangement => {
                let number = e.int() as usize;
                match number.checked_sub(1).and_then(|i| track_names.get(i)) {
                    Some((name, muted)) => {
                        let mut d = data.to_vec();
                        if *muted && let Some(b) = d.get_mut(12) {
                            *b = 0;
                        }
                        encode(e.id, &d, &mut body);
                        encode(EV_TRACK_NAME, &text(name), &mut body);
                    }
                    None => encode(e.id, data, &mut body),
                }
            }
            _ => encode(e.id, data, &mut body),
        }
    }
    let channel_count = channels.len().max(1) as u16;
    let mut out = Vec::with_capacity(body.len() + 22);
    out.extend_from_slice(b"FLhd");
    out.extend_from_slice(&6u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&channel_count.to_le_bytes());
    out.extend_from_slice(&(PPQ as u16).to_le_bytes());
    out.extend_from_slice(b"FLdt");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// FL's audio clip channel: the recipe save's third channel (number 2), up to the next channel or the arrangements,
/// without its sample's path and name.
fn audio_channel_block<'a, 'b>(all: &'b [Event<'a>]) -> Vec<&'b Event<'a>> {
    let Some(start) = all.iter().position(|e| e.id == EV_CHANNEL_NEW && e.int() == 2) else { return Vec::new() };
    all.iter()
        .skip(start)
        .enumerate()
        .take_while(|(i, e)| *i == 0 || !matches!(e.id, EV_CHANNEL_NEW | EV_ARRANGEMENT_NEW))
        .map(|(_, e)| e)
        .filter(|e| !matches!(e.id, EV_SAMPLE_PATH | EV_PLUGIN_NAME))
        .collect()
}

/// The playlist's items, in FL 25's 80 bytes each.
fn playlist(items: &[Item]) -> Vec<u8> {
    let mut out = Vec::with_capacity(items.len() * 80);
    for (i, item) in items.iter().enumerate() {
        let mut r = [0u8; 80];
        put(&mut r, 0, &item.at.to_le_bytes());
        put(&mut r, 4, &PATTERN_BASE.to_le_bytes());
        put(&mut r, 6, &item.what.to_le_bytes());
        put(&mut r, 8, &item.length.to_le_bytes());
        put(&mut r, 12, &item.track.to_le_bytes());
        put(&mut r, 16, &[0x78, 0x00, 0x40, 0x00, 0x40, 0x64, 0x80, 0x80]);
        let flags = 0x0040u16 | if item.muted { 0x2000 } else { 0 };
        put(&mut r, 18, &flags.to_le_bytes());
        match item.audio {
            Some((from, to, fade_in, fade_out)) => {
                // The whole file is -1.0 at both ends.
                let whole = from <= 0.0;
                put(&mut r, 24, &(if whole { -1.0f32 } else { from }).to_le_bytes());
                put(&mut r, 28, &(if whole { -1.0f32 } else { to }).to_le_bytes());
                put(&mut r, 36, &fade_in.max(0.0).to_le_bytes());
                put(&mut r, 44, &fade_out.max(0.0).to_le_bytes());
                put(&mut r, 52, &1.0f32.to_le_bytes());
                if fade_in > 0.0 || fade_out > 0.0 {
                    put(&mut r, 56, &[3]);
                }
            }
            None => put(&mut r, 24, &[0xff; 8]),
        }
        put(&mut r, 32, &((i + 1) as u32).to_le_bytes());
        put(&mut r, 64, &1.0f64.to_le_bytes());
        out.extend_from_slice(&r);
    }
    out
}
