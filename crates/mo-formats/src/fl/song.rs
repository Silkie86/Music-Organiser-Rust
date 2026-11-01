//! The song an FL project holds: its channels, what plays each one, and the notes and audio clips it plays where the
//! playlist puts them. This is what the MIDI page, the Overview and converting to another DAW are built on.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use super::automation::{
    FlAutomation, FlControl, FlPoint, MixerSettings, OnOff, TARGET_INSERT_PAN, TARGET_INSERT_VOLUME,
    TARGET_SLOT_ON_OFF, automation_points, place_automation, place_edit_events,
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
pub(super) const EV_ARRANGEMENT_NEW: u8 = 99;
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
pub(super) const EV_TRACK_DATA: u8 = 238;
/// A playlist track's name, after its settings (only when it was named).
pub(super) const EV_TRACK_NAME: u8 = 239;

pub(super) const CHANNEL_TYPE_AUTOMATION: u32 = 5;
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

impl FlStretch {
    /// How long the sample plays, in seconds, given its own length and the song's tempo: stretched to a length first
    /// when Time is set (FL counts 192 to a beat), then by Mul, then (when resampling) shortened as the pitch goes up.
    pub fn played_seconds(&self, seconds: f64, tempo: f64) -> f64 {
        let length = if self.time > 0 && tempo > 0.0 { self.time as f64 / 192.0 * 60.0 / tempo } else { seconds };
        let length = length * self.multiplier;
        if self.mode == 0 { length / 2f64.powf(self.pitch_cents as f64 / 1200.0) } else { length }
    }
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
    /// Where each mixer insert sends its sound (event 235), by insert: the inserts it goes to, the master being 0. An
    /// insert going to one other insert besides nothing else is a bus; several are sends.
    pub routes: BTreeMap<u32, Vec<u32>>,
    pub automation: Vec<FlAutomation>,
    /// The patterns placed in the song with edit events (knob moves recorded into the pattern, event 223) that aren't
    /// read: anything but a plugin's setting (those are in [`FlSong::automation`]), by number and name, so a converter
    /// can say they're left out.
    pub edit_events: Vec<(u32, String)>,
    /// Muted playlist tracks that have something on them: their clips are on a muted track, and their automation is
    /// left out.
    pub muted_tracks: Vec<FlTrack>,
    /// Each automation clip's own curve by its channel, in beats from the clip's start, before it's placed.
    pub curves: BTreeMap<u32, Vec<FlPoint>>,
    /// Said when the song places more notes ([`TOO_MANY_NOTES`]) or automation points ([`TOO_MANY_POINTS`]) than can
    /// be held: those up to there are kept.
    pub warning: Option<&'static str>,
}

/// How many notes the playlist may place in all: a pattern placed many times is a copy of its notes each time, so a
/// damaged or made-up file could otherwise ask for more memory than the computer has.
pub const MAX_PLACED_NOTES: usize = 4_000_000;

pub const TOO_MANY_NOTES: &str = "This song places more notes than the app holds (4 million), so only the song's \
     start was read: the clips after that are left out.";
pub const TOO_MANY_POINTS: &str = "This song's automation (its automation clips and the knob moves recorded into its \
     patterns) holds more points than the app holds (4 million), so some of it is left out.";

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
    /// How many tracks the playlist has, which a clip's slot counts down from (see [`top_slot`]): from FL 12.9.1
    /// always 500, whatever a save cut short kept of them; before that, told from the slots the clips are on.
    tracks_by_version: Option<i64>,
}

impl PlaylistTracks {
    /// Takes in the event if it's one of the first arrangement's tracks (or starts an arrangement).
    pub fn read(&mut self, e: &Event<'_>, flp: &Flp<'_>) {
        if self.tracks_by_version.is_none() && from_12_9_1(&flp.version) {
            self.tracks_by_version = Some(500);
        }
        match e.id {
            EV_ARRANGEMENT_NEW => self.arrangements += 1,
            EV_TRACK_DATA if self.arrangements <= 1 => {
                self.last = (e.data.len() > 12).then(|| e.int());
                if let Some(n) = self.last {
                    self.tracks.insert(n, (FlTrack { number: n, name: String::new() }, byte_at(&e.data, 12) == 0));
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

    /// Works out how many tracks a save from before FL 12.9.1 has, from the slots its clips are on: it doesn't keep
    /// every track (Solomon's FL 11 "Fuego96" keeps 35, with its clips on slots 189 to 198, tracks 1 to 10).
    pub fn count_from(&mut self, items: &[PlaylistItem], version: &str) {
        if self.tracks_by_version.is_none() {
            let highest = items.iter().map(|i| i.slot).max().unwrap_or(0);
            self.tracks_by_version = Some(top_slot(version, highest) + 1);
        }
    }

    /// The track a clip is on, from the slot FL saves with it, which counts down from the first track (499 is track 1
    /// of 500), and whether that track is muted.
    pub fn at(&self, slot: i64) -> Option<&(FlTrack, bool)> {
        let count = self.tracks_by_version.unwrap_or(500);
        u32::try_from(count - slot).ok().and_then(|n| self.tracks.get(&n))
    }

    /// The number of the track a clip's slot is, or 0 when the file doesn't say.
    pub fn number(&self, slot: i64) -> u32 {
        self.at(slot).map_or(0, |t| t.0.number)
    }

    fn muted(&self, slot: i64) -> bool {
        self.at(slot).is_some_and(|t| t.1)
    }
}

/// What FL saves for the first playlist track: tracks count down from it. From FL 12.9.1 (500 tracks) the first is
/// 499; before that 198 (199 tracks), though a 2012 song with all its clips on tracks 1 to 4 counted down from 98, so
/// an older song whose slots all fit under 99 is read that way (the C# app's arrangement preview, checked on Solomon's
/// saves from FL 9 to 12).
pub(super) fn top_slot(version: &str, highest: i64) -> i64 {
    if from_12_9_1(version) || highest > 198 {
        499
    } else if highest > 98 {
        198
    } else {
        98
    }
}

/// Whether FL's version ("25.1.0.4868") is 12.9.1 or later, when FL started saving 500 playlist tracks.
pub(super) fn from_12_9_1(version: &str) -> bool {
    let mut parts = version.split('.').map(|p| p.trim().parse::<u32>().ok());
    let mut next = || parts.next().flatten().unwrap_or(0);
    (next(), next(), next()) >= (12, 9, 1)
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

/// A clip's start offset at `at`, written as a float or an integer depending on FL's version; none is 0.
fn item_offset(d: &[u8], at: usize) -> f64 {
    let f = f32_at(d, at);
    if (1.0..1e9).contains(&f) {
        return f as f64;
    }
    let i = i32_at(d, at);
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
            offset: item_offset(r, 24),
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
                offset: item_offset(r, 24),
                muted: u16_at(r, 18) & 0x2000 != 0,
                slot: row as i64,
            });
        }
        let order: Vec<u32> = items.iter().map(|i| i.index).collect::<BTreeSet<_>>().into_iter().collect();
        for item in &mut items {
            item.slot = 499 - order.partition_point(|&i| i < item.index) as i64;
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
            // FL 6.0 writes 0x5000 where FL 6.0.4 to 8 write 0x6000 (checked on Solomon's 2006 saves).
            if kind == 0 && (index == 0x5000 || index == 0x6000) && row > FL8_ROWS && row < FL8_ROWS + 999 {
                index = PATTERN_BASE + FL8_ROWS + 999 - row;
            } else if kind != PATTERN_BASE || index >= PATTERN_BASE {
                return Vec::new();
            }
            items.push(PlaylistItem {
                index,
                at: u32_at(r, 0) as i64,
                length: u32_at(r, 8) as i64,
                offset: item_offset(r, 20),
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
            item.slot = 499 - order.partition_point(|&k| k < key) as i64;
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
) -> (BTreeMap<u32, Vec<FlClip>>, bool) {
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
    let mut placed_notes = 0usize;
    let mut cut = false;
    let mut place = |pattern: u32, at: i64, from: i64, length: i64, muted: bool, track: u32, on_muted_track: bool| {
        let (Some(in_pattern), Some(starts)) = (patterns.get(&pattern), by_start.get(&pattern)) else { return };
        let first = starts.partition_point(|&(start, _)| start < from);
        let end = starts.partition_point(|&(start, _)| start < from.saturating_add(length));
        placed_notes = placed_notes.saturating_add(end.saturating_sub(first));
        if cut || placed_notes > MAX_PLACED_NOTES {
            cut = true;
            return;
        }
        // In file order again: channels in the order their first note appears, as the C# reader groups them.
        let mut hits: Vec<usize> =
            starts.get(first..end.max(first)).unwrap_or_default().iter().map(|&(_, i)| i).collect();
        hits.sort_unstable();
        let mut groups: Vec<(u32, Vec<NoteEvent>)> = Vec::new();
        let mut group_of: HashMap<u32, usize> = HashMap::new();
        for &(channel, n) in hits.into_iter().filter_map(|i| in_pattern.get(i)) {
            let g = *group_of.entry(channel).or_insert_with(|| {
                groups.push((channel, Vec::new()));
                groups.len() - 1
            });
            if let Some((_, notes)) = groups.get_mut(g) {
                notes.push(NoteEvent {
                    start: n.start - from,
                    length: n.length.min(from.saturating_add(length) - n.start),
                    ..n
                });
            }
        }
        for (channel, mut notes) in groups {
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
    (clips, cut)
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
        .filter(|p| matches!(&*p.extension, ".fxp" | ".vstpreset"));
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
    let mut seen_channels: HashSet<u32> = HashSet::new();
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
    let mut edit_events: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    let mut pending_internal = String::new();
    let mut plugin_name = String::new();
    let mut insert_names: BTreeMap<u32, String> = BTreeMap::new();
    let mut insert_routes: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
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
                if seen_channels.insert(e.int()) {
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
            EV_PATTERN_CONTROLLERS if !e.data.is_empty() => {
                if let Some(p) = pattern {
                    edit_events.entry(p).or_default().extend_from_slice(&e.data);
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
            EV_INSERT_ROUTES if rack.in_mixer() => {
                if let Some(at) = rack.insert() {
                    let to = e.data.iter().enumerate().filter(|&(_, &b)| b != 0).map(|(i, _)| i as u32).collect();
                    insert_routes.insert(at, to);
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
    tracks.count_from(&items, &flp.version);
    let mixer = MixerSettings::new(mixer_settings);
    // An automation clip switched off in the Channel Rack doesn't play, like a muted one.
    links.retain(|(clip, _)| !off.contains(clip));
    let (mut automation, mut too_many_points, placed_points) =
        place_automation(&links, &curves, &names, &items, &tracks, &mixer, flp.ppq);
    // Knob moves recorded into patterns play wherever the pattern does, like an automation clip.
    let (recorded, edit_events, too_many_recorded) =
        place_edit_events(&edit_events, &names, &items, &tracks, &mixer, flp.ppq, placed_points);
    automation.extend(recorded);
    too_many_points |= too_many_recorded;
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
    let known = seen_channels;
    let (mut clips, too_many) = place_clips(flp, &raw_notes, &items, &tracks, &known, &pattern_names);
    // Only patterns the song plays (all of them where there's no playlist) lose anything: muted placements, and those
    // on a muted track, don't play.
    let placed: HashSet<u32> = items
        .iter()
        .filter(|i| i.index > PATTERN_BASE && !i.muted && !tracks.at(i.slot).is_some_and(|t| t.1))
        .map(|i| i.index - PATTERN_BASE)
        .collect();
    let edit_events: Vec<(u32, String)> = edit_events
        .into_iter()
        .filter(|p| items.is_empty() || placed.contains(p))
        .map(|p| (p, pattern_names.get(&p).cloned().unwrap_or_else(|| format!("Pattern {p}"))))
        .collect();
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
        routes: insert_routes,
        automation,
        edit_events,
        muted_tracks: muted_tracks.into_iter().collect(),
        curves: curves.into_iter().collect(),
        warning: if too_many { Some(TOO_MANY_NOTES) } else { too_many_points.then_some(TOO_MANY_POINTS) },
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
            // Each channel's track found once, not once per note: a pattern can hold millions of notes.
            let mut track_of: HashMap<u32, usize> = HashMap::new();
            for (c, note) in notes {
                let at = *track_of.entry(c).or_insert_with(|| {
                    let name = channel_names.get(&c).cloned().unwrap_or_else(|| format!("Channel {}", c + 1));
                    clip.track(&name);
                    clip.tracks.iter().position(|(n, _)| *n == name).unwrap_or(0)
                });
                if let Some((_, track)) = clip.tracks.get_mut(at) {
                    track.push(note);
                }
            }
            (!clip.tracks.is_empty()).then_some(clip)
        })
        .collect()
}

/// Each channel's notes across the song, where the playlist puts them (a song with nothing in the playlist gets its
/// patterns one after another), with what plays it. Unlike [`read_song`], automation clip channels and switched off
/// channels are kept, as the C# reader keeps them. Sample paths are as FL wrote them; [`resolve_channel_samples`]
/// finds those under FL's user data folder. A song that places more than [`MAX_PLACED_NOTES`] notes has the notes up to
/// there ([`read_song`] says so in its warning).
pub fn extract_channels(flp: &Flp<'_>) -> Vec<MidiChannel> {
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut sources: HashMap<u32, String> = HashMap::new();
    let mut players: HashMap<u32, String> = HashMap::new();
    let mut samples: HashMap<u32, String> = HashMap::new();
    let mut roots: HashMap<u32, u8> = HashMap::new();
    let mut pitches: HashMap<u32, i32> = HashMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut seen_channels: HashSet<u32> = HashSet::new();
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
                if seen_channels.insert(e.int()) {
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
    tracks.count_from(&items, &flp.version);
    let known = seen_channels;
    let notes: BTreeMap<u32, Vec<NoteEvent>> = place_clips(flp, &raw_notes, &items, &tracks, &known, &pattern_names)
        .0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fl_12_9_1_and_later_save_500_tracks() {
        assert!(from_12_9_1("12.9.1"));
        assert!(from_12_9_1("25.1.0.4868"));
        assert!(from_12_9_1("20.8"));
        assert!(!from_12_9_1("12.9.0.123"));
        assert!(!from_12_9_1("12.5.1"));
        assert!(!from_12_9_1(""));
        assert!(!from_12_9_1("garbled"));
    }

    #[test]
    fn a_save_cut_short_in_its_track_list_still_finds_the_first_track() {
        // Only tracks 1 and 2 of 500 were kept: slot 499 is still track 1.
        let mut tracks = PlaylistTracks { tracks_by_version: Some(500), ..PlaylistTracks::default() };
        for n in [1, 2] {
            tracks.tracks.insert(n, (FlTrack { number: n, name: String::new() }, false));
        }
        assert_eq!(tracks.number(499), 1);
        assert_eq!(tracks.number(498), 2);
        assert_eq!(tracks.number(1), 0, "track 499 wasn't kept");
        // An older save counts down from where its clips' slots say the first track is, however many it kept.
        tracks.tracks_by_version = None;
        let item = |slot| PlaylistItem { index: 1, at: 0, length: 96, offset: 0.0, muted: false, slot };
        tracks.count_from(&[item(198), item(197)], "11.0.3");
        assert_eq!(tracks.number(198), 1);
        assert_eq!(tracks.number(197), 2);
    }

    #[test]
    fn counts_tracks_down_from_the_top_slot_the_version_saves() {
        assert_eq!(top_slot("20.8.4.2576", 5), 499);
        assert_eq!(top_slot("12.9.1", 0), 499);
        assert_eq!(top_slot("12.5.1", 198), 198);
        assert_eq!(top_slot("11.0.0", 98), 98, "a 2012 song with its clips on tracks 1 to 4");
        assert_eq!(top_slot("9.0.0", 300), 499);
    }
}
