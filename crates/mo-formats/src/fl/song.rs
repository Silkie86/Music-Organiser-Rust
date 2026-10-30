//! The song an FL project holds: its channels, what plays each one, and the notes and audio clips it plays where the
//! playlist puts them. This is what the MIDI page, the Overview and converting to another DAW are built on.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use super::automation::{
    FlAutomation, FlControl, FlPoint, MixerSettings, OnOff, TARGET_INSERT_PAN, TARGET_INSERT_VOLUME,
    TARGET_SLOT_ON_OFF, automation_points, place_automation,
};
use super::events::*;
use super::notes;
use super::presets::{vst3_parameter_ids, wrapper_preset};
use super::{
    Rack, UserData, VST_PATH, VST_STATE, VST_VENDOR, WRAPPER, channels as known_channels, saved_nothing,
    wrapper_class_id, wrapper_fields, wrapper_four_cc, wrapper_is_vst3, wrapper_plugin_name, wrapper_text,
};
use crate::model::{MidiChannel, MidiClip, NoteEvent};
use crate::presets::PresetBlob;

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
/// The mixer's settings, 12 bytes each.
const EV_MIXER_SETTINGS: u8 = 225;
/// Which control an automation clip moves.
const EV_AUTOMATION_LINK: u8 = 227;
/// An automation clip's points.
const EV_AUTOMATION_POINTS: u8 = 234;
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
pub(super) const PATTERN_BASE: u32 = 20480;

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
    /// Its settings as a file the plugin can load (.fxp or .vstpreset), when there is one.
    pub state: Option<PresetBlob>,
    /// A VST3's own ids for its settings, in FL's order: FL automates setting n, other DAWs `parameter_ids[n]`.
    pub parameter_ids: Option<Vec<u32>>,
    /// An effect's slot in its mixer insert, from 0 at the top; `None` for an instrument, or when the file doesn't say.
    pub slot: Option<u32>,
    /// False for an effect switched off in its mixer slot (its green light is off).
    pub enabled: bool,
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
    /// Automation switches its channel off part of the way through it.
    pub partly_off: bool,
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
    /// False when it's switched off in the Channel Rack (its green light is off), so it plays nothing. A channel whose
    /// light is automated (`switched_by`) counts as on: what it plays while it's off is muted instead.
    pub enabled: bool,
    /// The automation clips that switch it on and off. Its clips that are off all the way through are muted, as are
    /// the notes in its other clips that start while it's off.
    pub switched_by: Vec<String>,
}

/// A mixer insert (0 is the master) and its effects in slot order.
#[derive(Clone, Debug, PartialEq)]
pub struct FlInsert {
    pub index: u32,
    pub name: String,
    pub effects: Vec<FlPlugin>,
    /// FL's fader value, when the file says: [`INSERT_FADER_DEFAULT`] where FL leaves it (how it maps to gain isn't
    /// worked out).
    pub fader: Option<i32>,
    /// FL's pan value, when the file says: 0 is the centre.
    pub pan: Option<i32>,
}

/// A mixer insert's fader where FL leaves it.
pub const INSERT_FADER_DEFAULT: i32 = 12800;

impl FlInsert {
    /// The fader or pan isn't where FL leaves it.
    pub fn moved(&self) -> bool {
        self.fader.is_some_and(|f| f != INSERT_FADER_DEFAULT) || self.pan.is_some_and(|p| p != 0)
    }
}

/// An FL project's song.
#[derive(Clone, Debug, PartialEq)]
pub struct FlSong {
    pub title: String,
    pub ppq: u16,
    pub tempo: f64,
    pub channels: Vec<FlChannel>,
    pub inserts: Vec<FlInsert>,
    pub automation: Vec<FlAutomation>,
    /// Muted playlist tracks that have something on them: their clips are on a muted track, and their automation is
    /// left out.
    pub muted_tracks: Vec<FlTrack>,
    /// Each automation clip's own curve by its channel, in beats from the clip's start, before it's placed.
    pub curves: BTreeMap<u32, Vec<FlPoint>>,
}

/// The first arrangement's playlist tracks and which are muted. FL saves every track, used or not, after its
/// arrangement's playlist (500 from FL 12.9), so their count is how many the playlist has. A track's settings (event
/// 238, 70 bytes in FL 25 and 26) start with u32 its number from 1, colour and icon, then a byte at 12 that's 1 when the
/// track plays and 0 when it's muted.
#[derive(Default)]
pub(super) struct PlaylistTracks {
    tracks: HashMap<u32, (FlTrack, bool)>,
    arrangements: u32,
    /// The track a name that follows is for: none after settings too short to read, so the name isn't put on the
    /// track before.
    last: Option<u32>,
}

impl PlaylistTracks {
    /// Takes in the event if it's one of the first arrangement's tracks (or starts an arrangement).
    pub fn read(&mut self, e: &Event<'_>, flp: &Flp<'_>) {
        match e.id {
            EV_ARRANGEMENT_NEW => self.arrangements += 1,
            EV_TRACK_DATA if self.arrangements <= 1 => {
                self.last = (e.data.len() > 12).then(|| e.int());
                if let Some(n) = self.last {
                    self.tracks.insert(n, (FlTrack { number: n, name: String::new() }, e.data[12] == 0));
                }
            }
            EV_TRACK_NAME if self.arrangements <= 1 => {
                if let Some(t) = self.last.and_then(|n| self.tracks.get_mut(&n)) {
                    t.0.name = flp.text(&e.data);
                }
            }
            _ => {}
        }
    }

    /// The track a clip is on, from the slot FL saves with it, which counts down from the first track (499 is track 1
    /// of 500), and whether that track is muted.
    pub fn at(&self, slot: i64) -> Option<&(FlTrack, bool)> {
        u32::try_from(self.tracks.len() as i64 - slot).ok().and_then(|n| self.tracks.get(&n))
    }

    /// The number of the track a clip's slot is, or 0 when the file doesn't say.
    pub fn number(&self, slot: i64) -> u32 {
        self.at(slot).map_or(0, |t| t.0.number)
    }

    fn muted(&self, slot: i64) -> bool {
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
    /// Can go below 0 in an old save with more than 500 patterns placed; no track is there.
    pub slot: i64,
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
            slot: u16_at(r, 12) as i64,
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
                slot: row as i64,
            });
        }
        let order: Vec<u32> = items.iter().map(|i| i.index).collect::<BTreeSet<_>>().into_iter().collect();
        for item in &mut items {
            item.slot = 499 - order.binary_search(&item.index).expect("listed above") as i64;
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
        let order: Vec<u32> = keys.iter().copied().collect::<BTreeSet<_>>().into_iter().collect();
        for (item, key) in items.iter_mut().zip(keys) {
            item.slot = 499 - order.binary_search(&key).expect("listed above") as i64;
        }
    }
    items
}

/// Each channel's clips across the song: one per pattern placed in the playlist (the first arrangement), holding the
/// channel's notes in that part of the pattern, timed from the clip's start. With nothing in the playlist (no pattern,
/// audio or automation clip), patterns play one after another, each rounded up to whole bars.
fn place_clips(
    flp: &Flp<'_>,
    raw_notes: &[(u32, &Event<'_>)],
    items: &[PlaylistItem],
    tracks: &PlaylistTracks,
    known: &HashSet<u32>,
    pattern_names: &HashMap<u32, String>,
) -> BTreeMap<u32, Vec<FlClip>> {
    let mut patterns: BTreeMap<u32, Vec<(u32, NoteEvent)>> = BTreeMap::new();
    for &(pattern, e) in raw_notes {
        let size = notes::note_size(&e.data, flp.major, e.offset.is_none(), Some(known));
        patterns.entry(pattern).or_default().extend(notes::decode(&e.data, size));
    }
    // Each pattern's notes by start (with their place in the file), so a clip finds its part of the pattern without
    // looking at every note: a drum pattern placed hundreds of times is read once.
    let by_start: HashMap<u32, Vec<(i64, usize)>> = patterns
        .iter()
        .map(|(&p, notes)| {
            let mut starts: Vec<(i64, usize)> = notes.iter().enumerate().map(|(i, (_, n))| (n.start, i)).collect();
            starts.sort_unstable();
            (p, starts)
        })
        .collect();
    let name_of = |p: u32| pattern_names.get(&p).cloned().unwrap_or_else(|| format!("Pattern {p}"));

    let mut clips: BTreeMap<u32, Vec<FlClip>> = BTreeMap::new();
    let mut place = |pattern: u32, at: i64, from: i64, length: i64, muted: bool, track: u32, on_muted_track: bool| {
        let (Some(in_pattern), Some(starts)) = (patterns.get(&pattern), by_start.get(&pattern)) else { return };
        let first = starts.partition_point(|&(start, _)| start < from);
        let end = starts.partition_point(|&(start, _)| start < from.saturating_add(length));
        // In file order again: channels in the order their first note appears, as the C# reader groups them.
        let mut hits: Vec<usize> = starts[first..end.max(first)].iter().map(|&(_, i)| i).collect();
        hits.sort_unstable();
        let mut by_channel: BTreeMap<u32, Vec<NoteEvent>> = BTreeMap::new();
        let mut order = Vec::new();
        for &(channel, n) in hits.into_iter().map(|i| &in_pattern[i]) {
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

    if !items.is_empty() {
        let mut patterns_placed: Vec<&PlaylistItem> = items.iter().filter(|i| i.index > PATTERN_BASE).collect();
        patterns_placed.sort_by_key(|i| i.at);
        for i in patterns_placed {
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
    items: &[PlaylistItem],
    tracks: &PlaylistTracks,
    channels: &HashSet<u32>,
) -> BTreeMap<u32, Vec<FlAudioClip>> {
    let mut clips: BTreeMap<u32, Vec<FlAudioClip>> = BTreeMap::new();
    let mut items: Vec<&PlaylistItem> =
        items.iter().filter(|i| i.index < PATTERN_BASE && channels.contains(&i.index) && i.length > 0).collect();
    items.sort_by_key(|i| i.at);
    for i in items {
        clips.entry(i.index).or_default().push(FlAudioClip {
            start: i.at,
            length: i.length,
            from: i.offset / 1000.0,
            muted: i.muted,
            track: tracks.number(i.slot),
            on_muted_track: tracks.muted(i.slot),
            partly_off: false,
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

/// The plugin a channel or slot holds. `shown` is the name the project gives it; with `sounds` false its settings
/// (which can be megabytes) are left out, for a quick look at what plays.
fn song_plugin(internal: &str, data: &[u8], shown: &str, sounds: bool) -> FlPlugin {
    if !internal.eq_ignore_ascii_case(WRAPPER) {
        return FlPlugin {
            name: internal.to_owned(),
            vendor: "Image-Line".into(),
            format: "FL",
            device_id: String::new(),
            path: String::new(),
            nothing_saved: false,
            state: None,
            parameter_ids: None,
            slot: None,
            enabled: true,
        };
    }
    let f = wrapper_fields(data);
    let path = wrapper_text(&f, VST_PATH);
    let name = wrapper_plugin_name(&f);
    let vst3 = wrapper_is_vst3(&f);
    let device_id = if vst3 {
        wrapper_class_id(&f).map(class_id_string).unwrap_or_default()
    } else {
        wrapper_four_cc(&f).map(|c| c.to_string()).unwrap_or_default()
    };
    let raw = f.get(&VST_STATE).copied().filter(|s| sounds && !s.is_empty());
    let nothing_saved = !vst3 && f.get(&VST_STATE).is_some_and(|s| saved_nothing(s));
    let state = raw
        .filter(|_| !nothing_saved)
        .map(|s| wrapper_preset(&f, s, if shown.is_empty() { &name } else { shown }, &name))
        // Raw states other DAWs can't load are left out.
        .filter(|p| matches!(p.extension, ".fxp" | ".vstpreset"));
    let parameter_ids = raw.filter(|_| vst3).and_then(vst3_parameter_ids);
    FlPlugin {
        name,
        vendor: wrapper_text(&f, VST_VENDOR),
        format: if vst3 { "VST3" } else { "VST2" },
        device_id,
        path,
        nothing_saved,
        state,
        parameter_ids,
        slot: None,
        enabled: true,
    }
}

impl FlSong {
    /// Finds samples saved under FL's user data folder from where the project lives (see [`UserData`]).
    pub fn resolve_samples(&mut self, project: &Path) {
        let mut user_data = UserData::new(project);
        for c in &mut self.channels {
            c.sample = user_data.resolve(&c.sample);
        }
    }
}

/// Finds the channels' samples saved under FL's user data folder from where the project lives.
pub fn resolve_channel_samples(channels: &mut [MidiChannel], project: &Path) {
    let mut user_data = UserData::new(project);
    for c in channels {
        c.sample_path = user_data.resolve(&c.sample_path);
    }
}

/// Reads the song: channels, what plays them, and their notes and audio clips where the playlist puts them. Sample
/// paths are as FL wrote them; [`FlSong::resolve_samples`] finds those under FL's user data folder.
pub fn read_song(flp: &Flp<'_>) -> FlSong {
    read_song_with(flp, true)
}

/// [`read_song`], with `sounds` false to leave each plugin's settings out (listing a project only asks what plays).
pub fn read_song_with(flp: &Flp<'_>, sounds: bool) -> FlSong {
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
    let mut rack = Rack::default();
    let mut pattern: Option<u32> = None;
    let mut pending_internal = String::new();
    let mut plugin_name = String::new();
    let mut insert_names: BTreeMap<u32, String> = BTreeMap::new();
    let mut effects: BTreeMap<u32, Vec<FlPlugin>> = BTreeMap::new();
    // The effect added last, until its slot's closing event says which slot it's in.
    let mut unslotted: Option<(u32, usize)> = None;
    let mut mixer_settings: Option<&[u8]> = None;
    let mut links: Vec<(u32, u32)> = Vec::new();
    let mut curves: HashMap<u32, Vec<FlPoint>> = HashMap::new();

    for e in &flp.events {
        tracks.read(e, flp);
        rack.step(e);
        let channel = rack.channel();
        match e.id {
            EV_TEMPO => tempo = Some(e.int() as f64 / 1000.0),
            EV_TEMPO_COARSE if tempo.is_none() => tempo = Some(e.int() as f64),
            EV_TITLE => title = flp.text(&e.data),
            EV_CHANNEL_NEW => {
                if !order.contains(&e.int()) {
                    order.push(e.int());
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
            EV_PLUGIN_INTERNAL => {
                pending_internal = flp.text(&e.data);
                plugin_name.clear();
            }
            EV_PLUGIN_NAME => plugin_name = flp.text(&e.data),
            EV_PLUGIN_STATE if !e.data.is_empty() && !pending_internal.is_empty() => {
                if let Some(c) = channel {
                    instruments
                        .entry(c)
                        .or_insert_with(|| song_plugin(&pending_internal, &e.data, &plugin_name, sounds));
                } else if rack.in_mixer()
                    && let Some(at) = rack.insert()
                {
                    let list = effects.entry(at).or_default();
                    list.push(song_plugin(&pending_internal, &e.data, &plugin_name, sounds));
                    unslotted = Some((at, list.len() - 1));
                }
                pending_internal.clear();
            }
            EV_INSERT_FLAGS => {
                if let (Some(at), Some(name)) = (rack.insert(), rack.insert_name(e).map(|n| flp.text(n)))
                    && !name.is_empty()
                {
                    insert_names.insert(at, name);
                }
            }
            EV_MIXER_SLOT => {
                // An effect's events come before the event that closes its slot, which holds the slot's number.
                if let Some((at, index)) = unslotted.take()
                    && let Some(fx) = effects.get_mut(&at).and_then(|l| l.get_mut(index))
                {
                    fx.slot = Some(e.int());
                }
            }
            EV_MIXER_SETTINGS => mixer_settings = Some(&e.data),
            EV_AUTOMATION_LINK if e.data.len() >= 12 => {
                links.push((u16_at(&e.data, 2) as u32, u32_at(&e.data, 8)));
            }
            _ => {}
        }
        // The rest are a channel's own events.
        let Some(c) = channel else { continue };
        match e.id {
            EV_CHANNEL_ON => _ = if e.int() == 0 { off.insert(c) } else { off.remove(&c) },
            EV_CHANNEL_INSERT | EV_CHANNEL_ROUTE => _ = routes.insert(c, e.int()),
            EV_CHANNEL_TYPE => _ = types.insert(c, e.int()),
            EV_ROOT_NOTE if e.int() <= 127 => _ = roots.insert(c, e.int() as u8),
            EV_CHANNEL_LEVELS => {
                if let Some(lv) = levels(&e.data, flp.major) {
                    levels_of.insert(c, lv);
                }
            }
            EV_ENVELOPE => {
                // The second of a channel's envelopes is its volume's.
                let seen = envelopes.entry(c).or_default();
                if *seen == 1
                    && let Some(env) = envelope(&e.data)
                {
                    volume_envelopes.insert(c, env);
                }
                *seen += 1;
            }
            EV_CHANNEL_PARAMS => {
                if let Some(s) = stretch(&e.data) {
                    stretches.insert(c, s);
                }
            }
            EV_CHANNEL_FX => _ = if e.int() & 0x2 != 0 { reversed.insert(c) } else { reversed.remove(&c) },
            EV_AUTOMATION_POINTS => _ = curves.insert(c, automation_points(&e.data)),
            EV_CHANNEL_NAME_OLD | EV_PLUGIN_NAME => _ = names.entry(c).or_insert_with(|| flp.text(&e.data)),
            EV_SAMPLE_PATH => {
                let sample = flp.text(&e.data);
                if !sample.is_empty() {
                    samples.insert(c, sample);
                }
            }
            _ => {}
        }
    }

    let items = playlist.map(playlist_items).unwrap_or_default();
    let mixer = MixerSettings::new(mixer_settings);
    // An automation clip switched off in the Channel Rack doesn't play, like a muted one.
    links.retain(|(clip, _)| !off.contains(clip));
    let automation = place_automation(&links, &curves, &names, &items, &tracks, &mixer, flp.ppq);
    // A channel whose on/off is automated plays only while it's on. When several clips switch it, it follows each in
    // turn: where one clip ends and another starts, the one starting there has the last word.
    // Per channel: the clips' names, and their points with the tick each is on and where its clip starts.
    type Switching = (Vec<String>, Vec<(i64, f64, FlPoint)>);
    let mut switching: BTreeMap<u32, Switching> = BTreeMap::new();
    for a in automation.iter().filter(|a| a.control == FlControl::OnOff) {
        let Some(c) = a.channel else { continue };
        let (names, points) = switching.entry(c).or_default();
        if !names.contains(&a.name) {
            names.push(a.name.clone());
        }
        let ppq = flp.ppq as f64;
        points.extend(a.points.iter().map(|&p| ((p.beat * ppq).round_ties_even() as i64, a.start_of(p), p)));
    }
    let switching: BTreeMap<u32, (Vec<String>, OnOff)> = switching
        .into_iter()
        .map(|(c, (names, mut points))| {
            points.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
            (c, (names, OnOff::new(points.into_iter().map(|p| p.2).collect(), flp.ppq)))
        })
        .collect();
    let known: HashSet<u32> = order.iter().copied().collect();
    let mut clips = place_clips(flp, &raw_notes, &items, &tracks, &known, &pattern_names);
    let playing: HashSet<u32> =
        order.iter().copied().filter(|c| types.get(c) != Some(&CHANNEL_TYPE_AUTOMATION)).collect();
    let mut audio_clips = place_audio_clips(&items, &tracks, &playing);
    let channels = order
        .iter()
        .copied()
        .filter(|c| playing.contains(c))
        .map(|c| {
            let (pan, volume, pitch_cents) = levels_of.get(&c).copied().unwrap_or((0.5, DEFAULT_VOLUME, 0));
            let mut placed = clips.remove(&c).unwrap_or_default();
            let mut placed_audio = audio_clips.remove(&c).unwrap_or_default();
            let switched = switching.get(&c);
            if let Some((_, on_off)) = switched {
                placed = placed.into_iter().map(|clip| on_off.mute(clip)).collect();
                placed_audio = placed_audio.into_iter().map(|clip| on_off.mute_audio(clip)).collect();
            }
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
                audio_clips: placed_audio,
                stretch: FlStretch {
                    reversed: reversed.contains(&c),
                    ..stretches.get(&c).copied().unwrap_or_default()
                },
                enabled: !off.contains(&c) || switched.is_some(),
                switched_by: switched.map(|s| s.0.clone()).unwrap_or_default(),
            }
        })
        .collect::<Vec<_>>();
    let muted_tracks: BTreeSet<FlTrack> =
        items.iter().filter_map(|i| tracks.at(i.slot)).filter(|(_, muted)| *muted).map(|(t, _)| t.clone()).collect();
    // An effect whose slot is switched on and off by automation stays on, as a channel whose light is automated does.
    let switched_slots: HashSet<(u32, u32)> = automation
        .iter()
        .filter(|a| a.control == FlControl::InsertSlotOnOff)
        .filter_map(|a| Some((a.insert?, a.slot?)))
        .collect();
    let numbers: BTreeSet<u32> =
        effects.keys().chain(insert_names.keys()).copied().chain(channels.iter().map(|c| c.insert)).collect();
    let inserts = numbers
        .into_iter()
        .map(|i| FlInsert {
            index: i,
            name: insert_names.get(&i).cloned().unwrap_or_default(),
            effects: effects
                .remove(&i)
                .unwrap_or_default()
                .into_iter()
                .map(|mut fx| {
                    if let Some(slot) = fx.slot
                        && mixer.value(i, slot, TARGET_SLOT_ON_OFF) == Some(0)
                        && !switched_slots.contains(&(i, slot))
                    {
                        fx.enabled = false;
                    }
                    fx
                })
                .collect(),
            fader: mixer.value(i, 0, TARGET_INSERT_VOLUME),
            pan: mixer.value(i, 0, TARGET_INSERT_PAN),
        })
        .collect();
    FlSong {
        title,
        ppq: flp.ppq,
        tempo: tempo.unwrap_or(140.0),
        channels,
        inserts,
        automation,
        muted_tracks: muted_tracks.into_iter().collect(),
        curves: curves.into_iter().collect(),
    }
}

/// One clip per pattern (in pattern order), one track in it per channel with notes in that pattern. Muted notes are
/// left out: MIDI has none.
pub fn extract_midi(flp: &Flp<'_>) -> Vec<MidiClip> {
    let known = known_channels(flp);
    let mut channel_names: HashMap<u32, String> = HashMap::new();
    let mut pattern_names: HashMap<u32, String> = HashMap::new();
    let mut pattern_notes: BTreeMap<u32, Vec<(u32, NoteEvent)>> = BTreeMap::new();
    let mut rack = Rack::default();
    let mut pattern: Option<u32> = None;
    for e in &flp.events {
        rack.step(e);
        match e.id {
            EV_PLUGIN_NAME | EV_CHANNEL_NAME_OLD => {
                if let Some(c) = rack.channel() {
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
                    let size = notes::note_size(&e.data, flp.major, e.offset.is_none(), Some(&known));
                    let unmuted = notes::decode(&e.data, size).into_iter().filter(|(_, n)| !n.muted);
                    pattern_notes.entry(p).or_default().extend(unmuted);
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
/// patterns one after another), with what plays it. Unlike [`read_song`], automation clip channels and switched off
/// channels are kept, as the C# reader keeps them. Sample paths are as FL wrote them; [`resolve_channel_samples`]
/// finds those under FL's user data folder.
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
    let mut rack = Rack::default();
    let mut pattern: Option<u32> = None;
    let mut pending_internal = String::new();

    for e in &flp.events {
        tracks.read(e, flp);
        rack.step(e);
        let channel = rack.channel();
        match e.id {
            EV_CHANNEL_NEW => {
                if !order.contains(&e.int()) {
                    order.push(e.int());
                }
            }
            EV_PLUGIN_NAME | EV_CHANNEL_NAME_OLD => {
                if let Some(c) = channel {
                    names.entry(c).or_insert_with(|| flp.text(&e.data));
                }
            }
            EV_PLUGIN_INTERNAL => {
                pending_internal = flp.text(&e.data);
                if let Some(c) = channel
                    && !pending_internal.is_empty()
                    && !pending_internal.eq_ignore_ascii_case(WRAPPER)
                {
                    sources.entry(c).or_insert_with(|| pending_internal.clone());
                    players.entry(c).or_insert_with(|| pending_internal.clone());
                }
            }
            EV_PLUGIN_STATE if pending_internal.eq_ignore_ascii_case(WRAPPER) => {
                if let Some(c) = channel {
                    let vst = wrapper_plugin_name(&wrapper_fields(&e.data));
                    if !vst.is_empty() {
                        sources.entry(c).or_insert_with(|| vst.clone());
                        players.entry(c).or_insert(vst);
                    }
                    pending_internal.clear();
                }
            }
            EV_SAMPLE_PATH => {
                if let Some(c) = channel {
                    let sample = flp.text(&e.data);
                    if !sample.is_empty() {
                        sources.insert(c, sample.rsplit(['\\', '/']).next().unwrap_or("").to_owned());
                        samples.insert(c, sample);
                    }
                }
            }
            EV_ROOT_NOTE => {
                if let Some(c) = channel.filter(|_| e.int() <= 127) {
                    roots.insert(c, e.int() as u8);
                }
            }
            EV_CHANNEL_LEVELS => {
                if let (Some(c), Some((_, _, pitch))) = (channel, levels(&e.data, flp.major)) {
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

    let items = playlist.map(playlist_items).unwrap_or_default();
    let known: HashSet<u32> = order.iter().copied().collect();
    let notes: BTreeMap<u32, Vec<NoteEvent>> = place_clips(flp, &raw_notes, &items, &tracks, &known, &pattern_names)
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
