//! The song an FL project holds: its channels, what plays each one, and the notes and audio clips it plays where the
//! playlist puts them. This is what the MIDI page, the Overview and converting to another DAW are built on.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::events::*;
use super::notes;
use super::{WRAPPER, vst2_header, wrapper_fields, wrapper_text};
use crate::model::{MidiChannel, MidiClip, NoteEvent};
use crate::plugins;

const EV_CHANNEL_ON: u8 = 0;
/// The mixer insert a channel is routed to (older versions).
const EV_CHANNEL_INSERT: u8 = 22;
/// Sample flags; 0x2 plays it backwards (PyFLP's reading, not yet seen in a save).
const EV_CHANNEL_FX: u8 = 70;
/// Starts an arrangement (FL 12.9 onwards); its name, playlist and tracks follow.
const EV_ARRANGEMENT_NEW: u8 = 99;
/// The mixer insert a channel is routed to (seen from FL 20 to 25).
const EV_CHANNEL_ROUTE: u8 = 104;
/// The channel's root key as a MIDI note; absent when it's C5 (60).
const EV_ROOT_NOTE: u8 = 135;
/// The sampler's settings, its time stretching among them.
const EV_CHANNEL_PARAMS: u8 = 215;
/// An envelope and LFO, one each for pan, volume, mod X, mod Y and pitch.
const EV_ENVELOPE: u8 = 218;
/// i32 pan (0..12800), volume (0..12800) and pitch in cents; pan and volume to 128 before FL 6.
const EV_CHANNEL_LEVELS: u8 = 219;
/// A playlist track's settings.
const EV_TRACK_DATA: u8 = 238;
/// A playlist track's name, after its settings (only when it was named).
const EV_TRACK_NAME: u8 = 239;

const CHANNEL_TYPE_AUTOMATION: u32 = 5;
const LEVELS_FULL: f64 = 12800.0;
/// A playlist item's index above this is a pattern (index − base); at or below, a channel's clip.
const PATTERN_BASE: u32 = 20480;

/// FL's default channel volume, 78% (10000 of 12800).
pub const DEFAULT_VOLUME: f64 = 10000.0 / LEVELS_FULL;

/// A plugin in an FL project, with what another DAW needs to load it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlPlugin {
    pub name: String,
    pub vendor: String,
    /// "VST2", "VST3", or "FL" for FL Studio's own plugins.
    pub format: &'static str,
    /// How other DAWs know the plugin: a VST2's id as a number, or a VST3's class id.
    pub device_id: String,
    /// Where it was installed on the PC that saved the project.
    pub path: String,
    /// The plugin gave FL nothing when the project was saved, so it opens at its default sound.
    pub nothing_saved: bool,
}

impl FlPlugin {
    pub fn native(&self) -> bool {
        self.format == "FL"
    }
}

/// A channel's time stretching (FL's Time, Pitch and Mul knobs, and its mode).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlStretch {
    /// The sample plays this many times as long.
    pub multiplier: f64,
    pub pitch_cents: i32,
    /// 0 resamples (pitch and length change together, like speeding up a record); the others keep them apart.
    pub mode: i32,
    /// Stretches it to a length first, in FL's units of 768 a bar; 0 when off.
    pub time: i32,
    pub reversed: bool,
}

impl Default for FlStretch {
    fn default() -> FlStretch {
        FlStretch { multiplier: 1.0, pitch_cents: 0, mode: 0, time: 0, reversed: false }
    }
}

/// An envelope in FL's own knob units: times from 100 to 65536 (how FL turns them into seconds isn't worked out yet),
/// and the sustain level from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlEnvelope {
    pub delay: i32,
    pub attack: i32,
    pub hold: i32,
    pub decay: i32,
    pub sustain: f64,
    pub release: i32,
}

/// One pattern placed in the playlist as one channel plays it: where it starts and how long it is (song ticks), and the
/// channel's notes in it, timed from the clip's start.
#[derive(Clone, Debug, PartialEq)]
pub struct FlClip {
    pub name: String,
    pub start: i64,
    pub length: i64,
    pub notes: Vec<NoteEvent>,
    pub muted: bool,
    /// The playlist track it's on, from 1 at the top; 0 when the file doesn't say or the song has no playlist.
    pub track: u32,
    /// That track is muted, so the clip doesn't play even when it isn't muted itself.
    pub on_muted_track: bool,
}

/// An audio clip in the playlist: where it starts and how long it plays (song ticks), and where in the sample it starts
/// playing, in seconds of the sample as recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct FlAudioClip {
    pub start: i64,
    pub length: i64,
    pub from: f64,
    pub muted: bool,
    pub track: u32,
    pub on_muted_track: bool,
}

/// A playlist track, numbered from 1 at the top, with its name when it was given one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FlTrack {
    pub number: u32,
    pub name: String,
}

impl FlTrack {
    /// How a note names it: "playlist track 4", or "Vocals, playlist track 4".
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            format!("playlist track {}", self.number)
        } else {
            format!("{}, playlist track {}", self.name, self.number)
        }
    }
}

/// A channel in the Channel Rack.
#[derive(Clone, Debug, PartialEq)]
pub struct FlChannel {
    pub index: u32,
    pub name: String,
    pub instrument: Option<FlPlugin>,
    pub sample: String,
    /// The mixer insert it feeds.
    pub insert: u32,
    /// What it plays across the song (song ticks): no muted notes, muted clips or clips on muted tracks.
    pub notes: Vec<NoteEvent>,
    /// The key its sample sounds at its own pitch on (60, FL's C5, unless changed).
    pub root_key: u8,
    /// Its pitch knob.
    pub pitch_cents: i32,
    /// FL's 0..1 knob positions.
    pub volume: f64,
    pub pan: f64,
    /// Its part of each pattern clip in the playlist, muted ones and muted notes included.
    pub clips: Vec<FlClip>,
    /// Its volume envelope, when that's switched on.
    pub volume_envelope: Option<FlEnvelope>,
    /// An audio clip channel's clips in the playlist, muted ones included, in order.
    pub audio_clips: Vec<FlAudioClip>,
    pub stretch: FlStretch,
    /// False when it's switched off in the Channel Rack (its green light is off), so it plays nothing.
    pub enabled: bool,
}

/// An FL project's song.
#[derive(Clone, Debug, PartialEq)]
pub struct FlSong {
    pub title: String,
    pub ppq: u16,
    pub tempo: f64,
    pub channels: Vec<FlChannel>,
    /// Muted playlist tracks that have something on them.
    pub muted_tracks: Vec<FlTrack>,
}

/// The first arrangement's playlist tracks and which are muted. FL saves every track, used or not, after its
/// arrangement's playlist (500 from FL 12.9), so their count is how many the playlist has. A track's settings (event
/// 238, 70 bytes in FL 25 and 26) start with u32 its number from 1, colour and icon, then a byte at 12 that's 1 when the
/// track plays and 0 when it's muted.
#[derive(Default)]
pub(super) struct PlaylistTracks {
    tracks: HashMap<u32, (FlTrack, bool)>,
    arrangements: u32,
    last: u32,
}

impl PlaylistTracks {
    /// Takes in the event if it's one of the first arrangement's tracks (or starts an arrangement).
    pub fn read(&mut self, e: &Event<'_>, flp: &Flp<'_>) {
        match e.id {
            EV_ARRANGEMENT_NEW => self.arrangements += 1,
            EV_TRACK_DATA if self.arrangements <= 1 && e.data.len() > 12 => {
                self.last = e.int();
                self.tracks.insert(self.last, (FlTrack { number: self.last, name: String::new() }, e.data[12] == 0));
            }
            EV_TRACK_NAME if self.arrangements <= 1 => {
                if let Some(t) = self.tracks.get_mut(&self.last) {
                    t.0.name = flp.text(&e.data);
                }
            }
            _ => {}
        }
    }

    /// The track a clip is on, from the slot FL saves with it, which counts down from the first track (499 is track 1
    /// of 500), and whether that track is muted.
    pub fn at(&self, slot: u32) -> Option<&(FlTrack, bool)> {
        (self.tracks.len() as i64 - slot as i64).try_into().ok().and_then(|n: u32| self.tracks.get(&n))
    }

    /// The number of the track a clip's slot is, or 0 when the file doesn't say.
    pub fn number(&self, slot: u32) -> u32 {
        self.at(slot).map_or(0, |t| t.0.number)
    }

    fn muted(&self, slot: u32) -> bool {
        self.at(slot).is_some_and(|t| t.1)
    }
}

/// A clip in the playlist, in ticks. `index` above 20480 is a pattern (index − 20480); otherwise a channel's clip.
/// `offset` is where the clip starts in what it plays: ticks into a pattern, milliseconds into an audio clip's sample.
/// `slot` is the playlist track it sits on as FL saves it, counting down from the first track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlaylistItem {
    pub index: u32,
    pub at: i64,
    pub length: i64,
    pub offset: f64,
    pub muted: bool,
    pub slot: u32,
}

fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([d[at], d[at + 1]])
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

/// A clip's start offset, written as a float or an integer depending on FL's version; none is 0.
fn item_offset(d: &[u8]) -> f64 {
    let f = f32::from_le_bytes([d[0], d[1], d[2], d[3]]);
    if (1.0..1e9).contains(&f) {
        return f as f64;
    }
    let i = i32_at(d, 0);
    if i > 0 && i < 1_000_000_000 { i as f64 } else { 0.0 }
}

/// The clips in a playlist event. FL 9 onwards writes 32-byte items (60 from FL 21, 80 from FL 25, 88 from FL 26):
/// u32 position, u16 pattern base (20480), u16 item index, u32 length, u16 slot at 12, u16 flags at 18 (0x2000 is
/// muted), start offset at 24. FL 9 and FL 6 to 8 lay theirs out differently (below). Older layouts aren't read.
pub(super) fn playlist_items(d: &[u8]) -> Vec<PlaylistItem> {
    let mut items = Vec::new();
    for size in [88, 80, 60, 32] {
        if d.is_empty() || !d.len().is_multiple_of(size) {
            continue;
        }
        if !d.chunks_exact(size).all(|r| u16_at(r, 4) as u32 == PATTERN_BASE) {
            continue;
        }
        items.extend(d.chunks_exact(size).map(|r| PlaylistItem {
            index: u16_at(r, 6) as u32,
            at: u32_at(r, 0) as i64,
            length: u32_at(r, 8) as i64,
            offset: item_offset(&r[24..]),
            muted: u16_at(r, 18) & 0x2000 != 0,
            slot: u16_at(r, 12) as u32,
        }));
        break;
    }
    // FL 9: 32-byte items with 0 and 0x6000 where newer FL has the pattern, and the playlist row, counted from 999
    // down, at 12. Each row plays one pattern there: row 999 − n is pattern n (checked against FL 25's re-save of funk
    // step_8). FL 25 gives each pattern played a track, in pattern order from track 1, so the slot is the one newer FL
    // saves for that track.
    if items.is_empty() && !d.is_empty() && d.len().is_multiple_of(32) {
        for r in d.as_chunks::<32>().0 {
            let row = u16_at(r, 12) as u32;
            if u16_at(r, 4) != 0 || u16_at(r, 6) != 0x6000 || row == 0 || row > 999 {
                // Not FL 9's: perhaps FL 6 to 8's, whose 28-byte items fill a whole number of 32 bytes too.
                items.clear();
                break;
            }
            items.push(PlaylistItem {
                index: PATTERN_BASE + 999 - row,
                at: u32_at(r, 0) as i64,
                length: u32_at(r, 8) as i64,
                offset: item_offset(&r[24..]),
                muted: u16_at(r, 18) & 0x2000 != 0,
                slot: row,
            });
        }
        let order: Vec<u32> =
            items.iter().map(|i| i.index).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        for item in &mut items {
            item.slot = 499 - order.binary_search(&item.index).expect("listed above") as u32;
        }
    }
    // FL 6 to 8: 28-byte items laid out like FL 9's, but with the start offset at 20 (a float, −1 = none) and rows
    // 29952 higher: pattern n plays on row 30951 − n. Audio and automation clips have the pattern base and their
    // channel instead, on rows counted down from 30818 below the patterns (checked on Solomon's FL 7 saves). Each
    // pattern gets a track, as for FL 9, and each row of audio or automation clips one after them.
    if items.is_empty() && !d.is_empty() && d.len().is_multiple_of(28) {
        const FL8_ROWS: u32 = 29952;
        let mut rows = Vec::new();
        for r in d.as_chunks::<28>().0 {
            let (kind, mut index, row) = (u16_at(r, 4) as u32, u16_at(r, 6) as u32, u16_at(r, 12) as u32);
            if kind == 0 && index == 0x6000 && row > FL8_ROWS && row < FL8_ROWS + 999 {
                index = PATTERN_BASE + FL8_ROWS + 999 - row;
            } else if kind != PATTERN_BASE || index >= PATTERN_BASE {
                return Vec::new();
            }
            items.push(PlaylistItem {
                index,
                at: u32_at(r, 0) as i64,
                length: u32_at(r, 8) as i64,
                offset: item_offset(&r[20..]),
                muted: false,
                slot: 0,
            });
            rows.push(row);
        }
        let keys: Vec<u32> = items
            .iter()
            .zip(&rows)
            .map(
                |(i, &row)| {
                    if i.index > PATTERN_BASE { i.index } else { PATTERN_BASE + 1000 + (u16::MAX as u32 - row) }
                },
            )
            .collect();
        let order: Vec<u32> = keys.iter().copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        for (item, key) in items.iter_mut().zip(keys) {
            item.slot = 499 - order.binary_search(&key).expect("listed above") as u32;
        }
    }
    items
}

/// Each channel's clips across the song: one per pattern placed in the playlist (the first arrangement), holding the
/// channel's notes in that part of the pattern, timed from the clip's start. With nothing in the playlist, patterns
/// play one after another, each rounded up to whole bars.
fn place_clips(
    flp: &Flp<'_>,
    raw_notes: &[(u32, &Event<'_>)],
    playlist: Option<&[u8]>,
    tracks: &PlaylistTracks,
    known: &HashSet<u32>,
    pattern_names: &HashMap<u32, String>,
) -> BTreeMap<u32, Vec<FlClip>> {
    let mut patterns: BTreeMap<u32, Vec<(u32, NoteEvent)>> = BTreeMap::new();
    for &(pattern, e) in raw_notes {
        let size = notes::note_size(&e.data, flp.major, e.offset.is_none(), Some(known));
        patterns.entry(pattern).or_default().extend(notes::decode(&e.data, size));
    }
    let name_of = |p: u32| pattern_names.get(&p).cloned().unwrap_or_else(|| format!("Pattern {p}"));

    let mut clips: BTreeMap<u32, Vec<FlClip>> = BTreeMap::new();
    let mut place = |pattern: u32, at: i64, from: i64, length: i64, muted: bool, track: u32, on_muted_track: bool| {
        let Some(in_pattern) = patterns.get(&pattern) else { return };
        let mut by_channel: BTreeMap<u32, Vec<NoteEvent>> = BTreeMap::new();
        // Channels in the order their first note appears, as the C# reader groups them.
        let mut order = Vec::new();
        for &(channel, n) in in_pattern.iter().filter(|(_, n)| n.start >= from && n.start < from + length) {
            if !by_channel.contains_key(&channel) {
                order.push(channel);
            }
            by_channel.entry(channel).or_default().push(NoteEvent {
                start: n.start - from,
                length: n.length.min(from + length - n.start),
                ..n
            });
        }
        for channel in order {
            let mut notes = by_channel.remove(&channel).expect("grouped above");
            notes.sort_by_key(|n| (n.start, n.key));
            clips.entry(channel).or_default().push(FlClip {
                name: name_of(pattern),
                start: at,
                length,
                notes,
                muted,
                track,
                on_muted_track,
            });
        }
    };

    let mut items: Vec<PlaylistItem> =
        playlist.map(playlist_items).unwrap_or_default().into_iter().filter(|i| i.index > PATTERN_BASE).collect();
    if !items.is_empty() {
        items.sort_by_key(|i| i.at);
        for i in items {
            place(
                i.index - PATTERN_BASE,
                i.at,
                i.offset as i64,
                i.length,
                i.muted,
                tracks.number(i.slot),
                tracks.muted(i.slot),
            );
        }
    } else {
        let bar = 4 * flp.ppq as i64;
        let mut at = 0;
        let lengths: Vec<(u32, i64)> = patterns
            .iter()
            .filter(|(_, notes)| !notes.is_empty())
            .map(|(&p, notes)| {
                let end = notes.iter().map(|(_, n)| n.start + n.length).max().unwrap_or(0);
                (p, bar.max((end + bar - 1).div_euclid(bar.max(1)) * bar))
            })
            .collect();
        for (pattern, length) in lengths {
            place(pattern, at, 0, length, false, 0, false);
            at += length;
        }
    }
    clips
}

/// Audio clips in the playlist (the first arrangement), by channel: items whose index is a channel. An audio clip's
/// start offset is in milliseconds of the sample, as FL 25 saves it.
fn place_audio_clips(
    playlist: Option<&[u8]>,
    tracks: &PlaylistTracks,
    channels: &HashSet<u32>,
) -> BTreeMap<u32, Vec<FlAudioClip>> {
    let mut clips: BTreeMap<u32, Vec<FlAudioClip>> = BTreeMap::new();
    let Some(playlist) = playlist else { return clips };
    let mut items: Vec<PlaylistItem> = playlist_items(playlist)
        .into_iter()
        .filter(|i| i.index < PATTERN_BASE && channels.contains(&i.index) && i.length > 0)
        .collect();
    items.sort_by_key(|i| i.at);
    for i in items {
        clips.entry(i.index).or_default().push(FlAudioClip {
            start: i.at,
            length: i.length,
            from: i.offset / 1000.0,
            muted: i.muted,
            track: tracks.number(i.slot),
            on_muted_track: tracks.muted(i.slot),
        });
    }
    clips
}

/// What plays of a channel's clips: the notes, less muted ones and those in muted clips or on muted tracks, in song
/// ticks.
fn audible(clips: &[FlClip]) -> Vec<NoteEvent> {
    let mut notes: Vec<NoteEvent> = clips
        .iter()
        .filter(|c| !c.muted && !c.on_muted_track)
        .flat_map(|c| c.notes.iter().filter(|n| !n.muted).map(|n| NoteEvent { start: c.start + n.start, ..*n }))
        .collect();
    notes.sort_by_key(|n| (n.start, n.key));
    notes
}

/// A channel's levels (event 219): pan and volume as 0..1 knob positions, and pitch in cents. FL 4 and 5 count pan
/// and volume to 128 (pan 64 is centre), FL 6 on to 12800: read with the newer scale, Solomon's FL 4 grimey came into
/// Bitwig with every track panned hard left and its fader at the bottom.
fn levels(d: &[u8], major: u32) -> Option<(f64, f64, i32)> {
    if d.len() < 12 {
        return None;
    }
    let full = if (1..6).contains(&major) { 128.0 } else { LEVELS_FULL };
    Some((
        (i32_at(d, 0) as f64 / full).clamp(0.0, 1.0),
        (i32_at(d, 4) as f64 / full).clamp(0.0, 1.0),
        i32_at(d, 8).clamp(-4800, 4800),
    ))
}

/// A channel's time stretching, from its sampler settings (event 215, 168 bytes in FL 25): i32 Time at 96, the pitch
/// in cents at 100, the multiplier at 104 (2 to the power raw / 10000, so 10000 is twice as long) and the mode at 108.
/// The multiplier, pitch and mode are checked against an FL 25 save; Time is PyFLP's reading.
fn stretch(d: &[u8]) -> Option<FlStretch> {
    if d.len() < 112 {
        return None;
    }
    Some(FlStretch {
        multiplier: 2f64.powf(i32_at(d, 104).clamp(-100_000, 100_000) as f64 / 10_000.0),
        pitch_cents: i32_at(d, 100).clamp(-4800, 4800),
        mode: i32_at(d, 108),
        time: i32_at(d, 96).max(0),
        reversed: false,
    })
}

/// An envelope (event 218, 68 bytes), or `None` when it's switched off: i32 flags, i32 on, then i32 delay, attack,
/// hold, decay, sustain (0..128) and release. The layout is PyFLP's; no real file with an envelope on is checked yet.
fn envelope(d: &[u8]) -> Option<FlEnvelope> {
    if d.len() < 32 || i32_at(d, 4) == 0 {
        return None;
    }
    let v = |i: usize| i32_at(d, i * 4);
    Some(FlEnvelope {
        delay: v(2),
        attack: v(3),
        hold: v(4),
        decay: v(5),
        sustain: (v(6) as f64 / 128.0).clamp(0.0, 1.0),
        release: v(7),
    })
}

/// The class id as a .vstpreset writes it: 32 hex digits in Windows GUID order (the first three groups byte-swapped).
pub fn class_id_string(tuid: &[u8; 16]) -> String {
    let mut b = *tuid;
    b[..4].reverse();
    b[4..6].reverse();
    b[6..8].reverse();
    b.iter().map(|x| format!("{x:02X}")).collect()
}

const VST_KIND: u32 = 50;
const VST_FOUR_CC: u32 = 51;
const VST_CLASS_ID: u32 = 52;
const VST_STATE: u32 = 53;
const VST_NAME: u32 = 54;
const VST_PATH: u32 = 55;
const VST_VENDOR: u32 = 56;
const KIND_VST3: u8 = 8;

fn song_plugin(internal: &str, data: &[u8]) -> FlPlugin {
    if !internal.eq_ignore_ascii_case(WRAPPER) {
        return FlPlugin {
            name: internal.to_owned(),
            vendor: "Image-Line".into(),
            format: "FL",
            device_id: String::new(),
            path: String::new(),
            nothing_saved: false,
        };
    }
    let f = wrapper_fields(data);
    let path = wrapper_text(&f, VST_PATH);
    let name = match wrapper_text(&f, VST_NAME) {
        n if n.is_empty() => plugins::name_from_path(&path),
        n => n,
    };
    let vst3 = f.get(&VST_KIND).and_then(|k| k.first()) == Some(&KIND_VST3) || path.to_lowercase().ends_with(".vst3");
    let device_id = if vst3 {
        f.get(&VST_CLASS_ID).and_then(|c| <&[u8; 16]>::try_from(*c).ok()).map(class_id_string).unwrap_or_default()
    } else {
        f.get(&VST_FOUR_CC)
            .and_then(|c| <[u8; 4]>::try_from(*c).ok())
            .map(|c| i32::from_le_bytes(c).to_string())
            .unwrap_or_default()
    };
    let nothing_saved =
        !vst3 && f.get(&VST_STATE).is_some_and(|s| matches!(vst2_header(s), Some((_, 0, _))) && s[4] & 0x08 != 0);
    FlPlugin {
        name,
        vendor: wrapper_text(&f, VST_VENDOR),
        format: if vst3 { "VST3" } else { "VST2" },
        device_id,
        path,
        nothing_saved,
    }
}

/// Reads the song: channels, what plays them, and their notes and audio clips where the playlist puts them.
/// Sample paths are as FL wrote them (see [`super::resolve_user_data`]).
pub fn read_song(flp: &Flp<'_>) -> FlSong {
    let mut title = String::new();
    let mut tempo: Option<f64> = None;
    let mut order: Vec<u32> = Vec::new();
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut samples: HashMap<u32, String> = HashMap::new();
    let mut routes: HashMap<u32, u32> = HashMap::new();
    let mut instruments: HashMap<u32, FlPlugin> = HashMap::new();
    let mut roots: HashMap<u32, u8> = HashMap::new();
    let mut levels_of: HashMap<u32, (f64, f64, i32)> = HashMap::new();
    let mut envelopes: HashMap<u32, u32> = HashMap::new();
    let mut volume_envelopes: HashMap<u32, FlEnvelope> = HashMap::new();
    let mut types: HashMap<u32, u32> = HashMap::new();
    let mut stretches: HashMap<u32, FlStretch> = HashMap::new();
    let mut reversed: HashSet<u32> = HashSet::new();
    let mut off: HashSet<u32> = HashSet::new();
    let mut raw_notes: Vec<(u32, &Event<'_>)> = Vec::new();
    let mut pattern_names: HashMap<u32, String> = HashMap::new();
    let mut playlist: Option<&[u8]> = None;
    let mut tracks = PlaylistTracks::default();
    let (mut channel, mut pattern): (Option<u32>, Option<u32>) = (None, None);
    let mut in_mixer = false;
    let mut channel_has_plugin = false;
    let mut pending_internal = String::new();

    for e in &flp.events {
        tracks.read(e, flp);
        // Most of a channel's events only count while its events are going (not in the mixer's).
        let on_channel = channel.filter(|_| !in_mixer);
        match e.id {
            EV_TEMPO => tempo = Some(e.int() as f64 / 1000.0),
            EV_TEMPO_COARSE if tempo.is_none() => tempo = Some(e.int() as f64),
            EV_TITLE => title = flp.text(&e.data),
            EV_CHANNEL_NEW => {
                let c = e.int();
                channel = Some(c);
                in_mixer = false;
                channel_has_plugin = false;
                if !order.contains(&c) {
                    order.push(c);
                }
            }
            EV_CHANNEL_ON => {
                if let Some(c) = on_channel {
                    if e.int() == 0 {
                        off.insert(c)
                    } else {
                        off.remove(&c)
                    };
                }
            }
            EV_CHANNEL_INSERT | EV_CHANNEL_ROUTE => {
                if let Some(c) = on_channel {
                    routes.insert(c, e.int());
                }
            }
            EV_CHANNEL_TYPE => {
                if let Some(c) = on_channel {
                    types.insert(c, e.int());
                }
            }
            EV_ROOT_NOTE => {
                if let Some(c) = on_channel.filter(|_| e.int() <= 127) {
                    roots.insert(c, e.int() as u8);
                }
            }
            EV_CHANNEL_LEVELS => {
                if let (Some(c), Some(lv)) = (on_channel, levels(&e.data, flp.major)) {
                    levels_of.insert(c, lv);
                }
            }
            EV_ENVELOPE => {
                if let Some(c) = on_channel {
                    // The second of a channel's envelopes is its volume's.
                    let seen = envelopes.entry(c).or_default();
                    if *seen == 1
                        && let Some(env) = envelope(&e.data)
                    {
                        volume_envelopes.insert(c, env);
                    }
                    *seen += 1;
                }
            }
            EV_CHANNEL_PARAMS => {
                if let (Some(c), Some(s)) = (on_channel, stretch(&e.data)) {
                    stretches.insert(c, s);
                }
            }
            EV_CHANNEL_FX => {
                if let Some(c) = on_channel {
                    if e.int() & 0x2 != 0 {
                        reversed.insert(c)
                    } else {
                        reversed.remove(&c)
                    };
                }
            }
            EV_CHANNEL_NAME_OLD => {
                if let Some(c) = on_channel {
                    names.entry(c).or_insert_with(|| flp.text(&e.data));
                }
            }
            EV_SAMPLE_PATH => {
                if let Some(c) = on_channel {
                    let sample = flp.text(&e.data);
                    if !sample.is_empty() {
                        samples.insert(c, sample);
                    }
                }
            }
            EV_INSERT_FLAGS | EV_MIXER_SLOT => in_mixer = true,
            EV_PLUGIN_INTERNAL => {
                pending_internal = flp.text(&e.data);
                if channel_has_plugin {
                    in_mixer = true;
                }
                channel_has_plugin = true;
            }
            EV_PLUGIN_NAME => {
                if let Some(c) = on_channel {
                    names.entry(c).or_insert_with(|| flp.text(&e.data));
                }
            }
            EV_PLUGIN_STATE if !e.data.is_empty() && !pending_internal.is_empty() => {
                if let Some(c) = channel.filter(|_| !in_mixer) {
                    instruments.entry(c).or_insert_with(|| song_plugin(&pending_internal, &e.data));
                }
                pending_internal.clear();
            }
            EV_PATTERN_NEW => pattern = Some(e.int()),
            EV_PATTERN_NAME => {
                if let Some(p) = pattern {
                    pattern_names.insert(p, flp.text(&e.data));
                }
            }
            EV_PATTERN_NOTES => {
                if let Some(p) = pattern {
                    raw_notes.push((p, e));
                }
            }
            EV_PLAYLIST if playlist.is_none() => playlist = Some(&e.data),
            _ => {}
        }
    }

    let known: HashSet<u32> = order.iter().copied().collect();
    let mut clips = place_clips(flp, &raw_notes, playlist, &tracks, &known, &pattern_names);
    let playing: HashSet<u32> =
        order.iter().copied().filter(|c| types.get(c) != Some(&CHANNEL_TYPE_AUTOMATION)).collect();
    let mut audio_clips = place_audio_clips(playlist, &tracks, &playing);
    let channels = order
        .iter()
        .copied()
        .filter(|c| playing.contains(c))
        .map(|c| {
            let (pan, volume, pitch_cents) = levels_of.get(&c).copied().unwrap_or((0.5, DEFAULT_VOLUME, 0));
            let placed = clips.remove(&c).unwrap_or_default();
            FlChannel {
                index: c,
                name: names.get(&c).cloned().unwrap_or_else(|| format!("Channel {}", c + 1)),
                instrument: instruments.remove(&c),
                sample: samples.remove(&c).unwrap_or_default(),
                insert: routes.get(&c).copied().unwrap_or(0),
                notes: audible(&placed),
                root_key: roots.get(&c).copied().unwrap_or(DEFAULT_ROOT as u8),
                pitch_cents,
                volume,
                pan,
                clips: placed,
                volume_envelope: volume_envelopes.get(&c).copied(),
                audio_clips: audio_clips.remove(&c).unwrap_or_default(),
                stretch: FlStretch {
                    reversed: reversed.contains(&c),
                    ..stretches.get(&c).copied().unwrap_or_default()
                },
                enabled: !off.contains(&c),
            }
        })
        .collect();
    let mut muted_tracks: Vec<FlTrack> = playlist
        .map(playlist_items)
        .unwrap_or_default()
        .iter()
        .filter_map(|i| tracks.at(i.slot))
        .filter(|(_, muted)| *muted)
        .map(|(t, _)| t.clone())
        .collect();
    muted_tracks.sort();
    muted_tracks.dedup();
    FlSong { title, ppq: flp.ppq, tempo: tempo.unwrap_or(140.0), channels, muted_tracks }
}

/// One clip per pattern (in pattern order), one track in it per channel with notes in that pattern. Muted notes are
/// left out: MIDI has none.
pub fn extract_midi(flp: &Flp<'_>) -> Vec<MidiClip> {
    let mut channel_names: HashMap<u32, String> = HashMap::new();
    let mut pattern_names: HashMap<u32, String> = HashMap::new();
    let mut pattern_notes: BTreeMap<u32, Vec<(u32, NoteEvent)>> = BTreeMap::new();
    let (mut channel, mut pattern): (Option<u32>, Option<u32>) = (None, None);
    let mut in_mixer = false;
    for e in &flp.events {
        match e.id {
            EV_CHANNEL_NEW => {
                channel = Some(e.int());
                in_mixer = false;
            }
            EV_MIXER_SLOT | EV_INSERT_FLAGS => in_mixer = true,
            EV_PLUGIN_NAME | EV_CHANNEL_NAME_OLD if !in_mixer => {
                if let Some(c) = channel {
                    channel_names.entry(c).or_insert_with(|| flp.text(&e.data));
                }
            }
            EV_PATTERN_NEW => pattern = Some(e.int()),
            EV_PATTERN_NAME => {
                if let Some(p) = pattern {
                    pattern_names.insert(p, flp.text(&e.data));
                }
            }
            EV_PATTERN_NOTES => {
                if let Some(p) = pattern {
                    let size = notes::note_size(&e.data, flp.major, e.offset.is_none(), None);
                    pattern_notes
                        .entry(p)
                        .or_default()
                        .extend(notes::decode(&e.data, size).into_iter().filter(|(_, n)| !n.muted));
                }
            }
            _ => {}
        }
    }
    pattern_notes
        .into_iter()
        .filter_map(|(p, notes)| {
            let mut clip = MidiClip {
                name: pattern_names.get(&p).cloned().unwrap_or_else(|| format!("Pattern {p}")),
                ppq: flp.ppq,
                ..MidiClip::default()
            };
            for (c, note) in notes {
                clip.track(&channel_names.get(&c).cloned().unwrap_or_else(|| format!("Channel {}", c + 1))).push(note);
            }
            (!clip.tracks.is_empty()).then_some(clip)
        })
        .collect()
}

/// Each channel's notes across the song, where the playlist puts them (a song with nothing in the playlist gets its
/// patterns one after another), with what plays it. Sample paths are as FL wrote them.
pub fn extract_channels(flp: &Flp<'_>) -> Vec<MidiChannel> {
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut sources: HashMap<u32, String> = HashMap::new();
    let mut players: HashMap<u32, String> = HashMap::new();
    let mut samples: HashMap<u32, String> = HashMap::new();
    let mut roots: HashMap<u32, u8> = HashMap::new();
    let mut pitches: HashMap<u32, i32> = HashMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut raw_notes: Vec<(u32, &Event<'_>)> = Vec::new();
    let mut pattern_names: HashMap<u32, String> = HashMap::new();
    let mut playlist: Option<&[u8]> = None;
    let mut tracks = PlaylistTracks::default();
    let (mut channel, mut pattern): (Option<u32>, Option<u32>) = (None, None);
    let mut in_mixer = false;
    let mut channel_has_plugin = false;
    let mut pending_internal = String::new();

    for e in &flp.events {
        tracks.read(e, flp);
        let on_channel = channel.filter(|_| !in_mixer);
        match e.id {
            EV_CHANNEL_NEW => {
                let c = e.int();
                channel = Some(c);
                in_mixer = false;
                channel_has_plugin = false;
                if !order.contains(&c) {
                    order.push(c);
                }
            }
            EV_MIXER_SLOT | EV_INSERT_FLAGS => in_mixer = true,
            EV_PLUGIN_NAME | EV_CHANNEL_NAME_OLD => {
                if let Some(c) = on_channel {
                    names.entry(c).or_insert_with(|| flp.text(&e.data));
                }
            }
            EV_PLUGIN_INTERNAL => {
                pending_internal = flp.text(&e.data);
                if channel_has_plugin {
                    in_mixer = true;
                }
                channel_has_plugin = true;
                if let Some(c) = channel.filter(|_| !in_mixer)
                    && !pending_internal.is_empty()
                    && !pending_internal.eq_ignore_ascii_case(WRAPPER)
                {
                    sources.entry(c).or_insert_with(|| pending_internal.clone());
                    players.entry(c).or_insert_with(|| pending_internal.clone());
                }
            }
            EV_PLUGIN_STATE if pending_internal.eq_ignore_ascii_case(WRAPPER) => {
                if let Some(c) = on_channel {
                    let f = wrapper_fields(&e.data);
                    let vst = match wrapper_text(&f, VST_NAME) {
                        n if n.is_empty() => plugins::name_from_path(&wrapper_text(&f, VST_PATH)),
                        n => n,
                    };
                    if !vst.is_empty() {
                        sources.entry(c).or_insert_with(|| vst.clone());
                        players.entry(c).or_insert(vst);
                    }
                    pending_internal.clear();
                }
            }
            EV_SAMPLE_PATH => {
                if let Some(c) = on_channel {
                    let sample = flp.text(&e.data);
                    if !sample.is_empty() {
                        sources.insert(c, sample.rsplit(['\\', '/']).next().unwrap_or("").to_owned());
                        samples.insert(c, sample);
                    }
                }
            }
            EV_ROOT_NOTE => {
                if let Some(c) = on_channel.filter(|_| e.int() <= 127) {
                    roots.insert(c, e.int() as u8);
                }
            }
            EV_CHANNEL_LEVELS => {
                if let (Some(c), Some((_, _, pitch))) = (on_channel, levels(&e.data, flp.major)) {
                    pitches.insert(c, pitch);
                }
            }
            EV_PATTERN_NEW => pattern = Some(e.int()),
            EV_PATTERN_NAME => {
                if let Some(p) = pattern {
                    pattern_names.insert(p, flp.text(&e.data));
                }
            }
            EV_PATTERN_NOTES => {
                if let Some(p) = pattern {
                    raw_notes.push((p, e));
                }
            }
            EV_PLAYLIST if playlist.is_none() => playlist = Some(&e.data),
            _ => {}
        }
    }

    let known: HashSet<u32> = order.iter().copied().collect();
    let notes: BTreeMap<u32, Vec<NoteEvent>> = place_clips(flp, &raw_notes, playlist, &tracks, &known, &pattern_names)
        .into_iter()
        .map(|(c, clips)| (c, audible(&clips)))
        .filter(|(_, notes)| !notes.is_empty())
        .collect();
    // The rack's order first, then any channel that only shows up in notes.
    let mut channels = order.clone();
    channels.extend(notes.keys().filter(|c| !known.contains(c)));
    channels
        .into_iter()
        .filter_map(|c| {
            let notes = notes.get(&c)?.clone();
            Some(MidiChannel {
                name: names.get(&c).cloned().unwrap_or_else(|| format!("Channel {}", c + 1)),
                source: sources.get(&c).cloned().unwrap_or_default(),
                ppq: flp.ppq,
                notes,
                plugin: players.get(&c).cloned().unwrap_or_default(),
                sample_path: samples.get(&c).cloned().unwrap_or_default(),
                root_key: roots.get(&c).copied().unwrap_or(DEFAULT_ROOT as u8),
                pitch_cents: pitches.get(&c).copied().unwrap_or(0),
                channel: Some(c),
            })
        })
        .collect()
}
