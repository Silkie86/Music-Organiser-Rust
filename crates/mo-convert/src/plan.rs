//! What an FL song becomes in another DAW, in no DAW's terms (the choices the C# `BitwigProject.FromFl` makes, apart
//! from writing Bitwig's objects).
//!
//! Each FL channel that plays notes becomes a track of its own, as tracks are in Bitwig, with a clip for each FL
//! pattern clip it plays in and the third-party effects of the FL mixer insert it feeds after its instrument; an audio
//! clip channel's clips go on audio tracks. The master insert's effects go on the master. FL Studio's own plugins run
//! only in FL, so they're left out and listed.

use std::collections::{BTreeMap, HashMap, HashSet};

use mo_formats::fl::automation::{FlAutomation, FlControl, FlPoint};
use mo_formats::fl::song::{self, FlAudioClip, FlChannel, FlClip, FlInsert, FlPlugin, FlSong, FlStretch};

use crate::clips;
use crate::notes;

/// FL plays a plugin's root key as C5 (MIDI 60).
pub const DEFAULT_ROOT: u8 = 60;

/// What a DAW records about a sample file: its size and MD5 (how Bitwig recognises the file) and its format.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleFile {
    pub bytes: u64,
    pub md5: [u8; 16],
    pub rate: u32,
    pub channels: u16,
    /// 0 when the file could be read but not decoded (a format the app doesn't play).
    pub seconds: f64,
}

/// What the target DAW's writer can carry, and how steps are timed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Steps from FL's step sequencer on a sampler last until the next note on their key or the end of their clip, as
    /// on a plugin track. Otherwise they also end when their sample does.
    pub steps_until_next_note: bool,
    /// The writer can make audio tracks (Bitwig's needs one with an audio clip in the template).
    pub audio_tracks: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { steps_until_next_note: false, audio_tracks: true }
    }
}

/// An instrument or an effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Instrument,
    Effect,
}

/// A device on a track, in the order the sound goes through them.
#[derive(Clone, Debug, PartialEq)]
pub enum Device {
    /// A sampler playing the channel's sample: its root key, and its tune (FL's pitch knob) in semitones.
    Sampler { sample: String, info: Option<SampleFile>, root_key: u8, tune: f64 },
    /// Shifts notes by whole octaves (-3 to 3) and then semitones (-48 to 48) the same way, so -4 is 4 semitones down
    /// rather than an octave down and 8 up.
    Transpose { octaves: i32, semitones: i32 },
    /// A VST2 or VST3 plugin with its settings. `enabled` is false for an effect switched off in its FL mixer slot;
    /// `insert` names the FL mixer insert an effect comes from ("" for an instrument).
    Plugin { plugin: Box<FlPlugin>, role: Role, enabled: bool, insert: String },
}

/// What a track is made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    /// Plays the channel's sample.
    Sampler,
    /// Plays notes on a plugin (or nothing, when the channel's plugin is one of FL's own).
    Instrument,
    /// Holds audio clips.
    Audio,
}

/// An audio clip, in beats: where it starts and how long it plays; where in the clip's audio it starts playing; and
/// the clip's audio, the whole sample stretched to play `played` beats and transposed `semitones`, as in FL.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioClip {
    pub start: f64,
    pub length: f64,
    pub offset: f64,
    pub played: f64,
    /// How long the sample is, in its own seconds: a guess from the clip when the file couldn't be read.
    pub seconds: f64,
    pub semitones: f64,
    pub muted: bool,
}

/// A track.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub name: String,
    /// The FL channel's name (an audio channel's second track is "Vox 2", its source still "Vox").
    pub source: String,
    /// The FL channel it comes from.
    pub channel: u32,
    pub kind: TrackKind,
    pub devices: Vec<Device>,
    /// Pattern clips, in song ticks at the plan's PPQ, as one track holds them (see [`clips::of`]).
    pub clips: Vec<FlClip>,
    pub audio_clips: Vec<AudioClip>,
    /// For an audio track, the sample its clips play, and what's known of it.
    pub sample: Option<(String, Option<SampleFile>)>,
    /// A linear gain from 0 to 2 (1 is FL's default level).
    pub gain: f64,
    /// -1 (left) to 1 (right).
    pub pan: f64,
    pub muted: bool,
}

/// The master track, or a track by its place in [`Plan::tracks`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrackRef {
    Master,
    Track(usize),
}

/// What an automation lane moves.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    /// The track's volume, as a linear gain from 0 to 2.
    Volume,
    /// The track's pan, -1 to 1.
    Pan,
    /// A plugin's setting, 0 to 1: the device by its place on the track, the setting by its path in the plugin and the
    /// plugin's own id for it, and the name FL gave it.
    Plugin { device: usize, path: String, id: u32, name: String },
}

impl Target {
    /// Moves the same thing: a plugin's setting is the same whatever FL called the clips moving it.
    pub fn same_as(&self, other: &Target) -> bool {
        match (self, other) {
            (Target::Plugin { device, path, .. }, Target::Plugin { device: d, path: p, .. }) => {
                device == d && path == p
            }
            _ => self == other,
        }
    }
}

/// A point on a lane: beats from the song's start, the value in the target's units, and where the FL clip it came from
/// starts (points are joined by straight lines).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LanePoint {
    pub beat: f64,
    pub value: f64,
    pub from: f64,
}

/// An automation lane: every FL clip moving one thing on one track, in song order.
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    /// The FL automation clips it's made from, so a writer that can't carry it can say which.
    pub clips: Vec<String>,
    pub track: TrackRef,
    pub target: Target,
    pub points: Vec<LanePoint>,
}

/// What the song becomes.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub title: String,
    pub tempo: f64,
    pub ppq: u16,
    pub tracks: Vec<Track>,
    /// The master insert's effects, after what's on the target's master.
    pub master_effects: Vec<Device>,
    /// One lane per thing moved, in the order they were first met.
    pub lanes: Vec<Lane>,
    /// What couldn't come across, each said once.
    pub notes: Vec<String>,
}

/// FL's channel volume knob (0 to 1, 0 dB at its default 78%) as a gain for a 0 to 2 linear fader. FL's curve isn't
/// published; a square law keeps each channel's level relative to the default (as the C# app does).
pub fn gain(fl_volume: f64) -> f64 {
    (fl_volume / song::DEFAULT_VOLUME).powi(2).min(2.0)
}

/// FL's pan knob (0 to 1, centre 0.5) as -1 to 1.
pub fn pan(fl_pan: f64) -> f64 {
    ((fl_pan - 0.5) * 2.0).clamp(-1.0, 1.0)
}

/// A shift in semitones as whole octaves (up to 3 either way) and the rest in semitones.
pub fn transpose(semitones: i32) -> Device {
    let octaves = (semitones / 12).clamp(-3, 3);
    Device::Transpose { octaves, semitones: semitones.saturating_sub(octaves * 12).clamp(-48, 48) }
}

/// How long, in ticks, a sample of `seconds` plays when pitched `semitones` up.
pub fn sample_ticks(seconds: f64, semitones: f64, tempo: f64, ppq: u16) -> i64 {
    // A float to int cast stops at i64's ends rather than overflowing, which is what a silly sample length needs.
    (seconds / 2f64.powf(semitones / 12.0) * tempo / 60.0 * ppq as f64 - 1e-6).ceil() as i64
}

/// FL's Time knob counts 192 a beat.
const TIME_PER_BEAT: f64 = 192.0;

/// How many seconds a sample `seconds` long plays for at `tempo`, stretched as the channel stretches it.
pub fn played_seconds(stretch: &FlStretch, seconds: f64, tempo: f64) -> f64 {
    let length =
        if stretch.time > 0 && tempo > 0.0 { stretch.time as f64 / TIME_PER_BEAT * 60.0 / tempo } else { seconds };
    let length = length * stretch.multiplier;
    if stretch.mode == 0 { length / 2f64.powf(stretch.pitch_cents as f64 / 1200.0) } else { length }
}

/// How many semitones higher than recorded a sample `seconds` long sounds when it plays for `played` seconds.
pub fn stretched_semitones(stretch: &FlStretch, seconds: f64, played: f64) -> f64 {
    if stretch.mode == 0 && seconds > 0.0 && played > 0.0 {
        -12.0 * (played / seconds).log2()
    } else {
        stretch.pitch_cents as f64 / 100.0
    }
}

/// How another DAW names a plugin's setting: a VST2 setting by its number, which FL uses too; a VST3 one by the
/// plugin's own id for it, from the list FL keeps (FL automates setting n, other DAWs id n of that list).
pub fn setting_path(plugin: &FlPlugin, setting: u32) -> Option<(String, u32)> {
    match plugin.format {
        "VST2" => Some((format!("CONTENTS/ROOT_GENERIC_MODULE/PARAM{setting}"), setting)),
        "VST3" => {
            let id = *plugin.parameter_ids.as_ref()?.get(setting as usize)?;
            Some((format!("CONTENTS/ROOT_GENERIC_MODULE/PID{id:x}"), id))
        }
        _ => None,
    }
}

/// FL names an automation clip "channel - setting".
fn setting_name(clip: &str, channel: &str) -> String {
    clip.strip_prefix(channel).and_then(|r| r.strip_prefix(" - ")).unwrap_or(clip).to_owned()
}

/// Each copy of an insert's effect, by insert and slot: the track it's on, its place there, and the plugin.
type EffectCopies = HashMap<(u32, u32), Vec<(TrackRef, usize, FlPlugin)>>;

struct Planner<'a> {
    song: &'a FlSong,
    options: Options,
    read_sample: &'a mut dyn FnMut(&str) -> Option<SampleFile>,
    inserts: HashMap<u32, &'a FlInsert>,
    shared: HashSet<u32>,
    notes: Vec<String>,
    effect_copies: EffectCopies,
    /// How many of each channel's tracks have their own copy of its insert's effects.
    tracks_with_effects: HashMap<u32, usize>,
}

impl Planner<'_> {
    /// A plugin as a device, when another DAW can load it; what's left out of it goes in `skipped`.
    fn plugin(p: &FlPlugin, role: Role, skipped: &mut Vec<String>) -> Option<Device> {
        if p.native() || !matches!(p.format, "VST2" | "VST3") {
            skipped.push(format!("{} (FL Studio plugin)", p.name));
            return None;
        }
        if p.state.is_none() {
            if p.nothing_saved {
                skipped.push(format!(
                    "{}'s sound (FL Studio saved none in this file, so it opens at its default)",
                    p.name
                ));
            } else {
                skipped.push(format!("{}'s settings (opens at its defaults)", p.name));
            }
        }
        Some(Device::Plugin {
            plugin: Box::new(p.clone()),
            role,
            enabled: role == Role::Instrument || p.enabled,
            insert: String::new(),
        })
    }

    /// An insert's third-party effects, as devices on a track whose devices so far are `before`; each copy is kept
    /// for automating its settings.
    fn effects(
        &mut self,
        insert: Option<&FlInsert>,
        track: TrackRef,
        before: usize,
        skipped: &mut Vec<String>,
    ) -> Vec<Device> {
        let mut made = Vec::new();
        let Some(insert) = insert else { return made };
        for effect in &insert.effects {
            let Some(mut device) = Self::plugin(effect, Role::Effect, skipped) else { continue };
            if let Device::Plugin { insert: name, .. } = &mut device {
                *name = notes::insert_name(insert);
            }
            made.push(device);
            if let Some(slot) = effect.slot {
                self.effect_copies.entry((insert.index, slot)).or_default().push((
                    track,
                    before + made.len() - 1,
                    effect.clone(),
                ));
            }
        }
        made
    }

    /// The effects of the FL insert a channel feeds, after what's on its track.
    fn insert_effects(&mut self, c: &FlChannel, track: TrackRef, devices: &mut Vec<Device>, skipped: &mut Vec<String>) {
        if c.insert == 0 {
            return;
        }
        let Some(&insert) = self.inserts.get(&c.insert) else { return };
        let effects = self.effects(Some(insert), track, devices.len(), skipped);
        if effects.is_empty() {
            return;
        }
        devices.extend(effects);
        *self.tracks_with_effects.entry(c.index).or_default() += 1;
        if self.shared.contains(&c.insert) {
            self.notes.push(format!(
                "{}: has its own copy of {}'s effects, which other channels also feed in FL Studio",
                c.name,
                notes::insert_name(insert)
            ));
        }
    }

    /// A channel that plays notes: a track with its sampler or plugin, and a clip for each FL pattern clip. Returns the
    /// track and, when it has a plugin, the plugin's place on it.
    fn instrument_track(&mut self, c: &FlChannel, at: usize) -> (Track, Option<(usize, FlPlugin)>) {
        let song = self.song;
        let mut skipped = Vec::new();
        let mut devices = Vec::new();
        let mut instrument = None;
        let mut sample_length: Option<Box<dyn Fn(u8) -> i64>> = None;
        let kind = match &c.instrument {
            None => {
                let info = (self.read_sample)(&c.sample);
                match &info {
                    None => self
                        .notes
                        .push(format!("{}: couldn't read {}, so Bitwig will ask where it is", c.name, c.sample)),
                    Some(i) if i.seconds <= 0.0 => self.notes.push(format!(
                        "{}: couldn't tell how long {} is, so Bitwig will ask where it is",
                        c.name, c.sample
                    )),
                    Some(i) if !self.options.steps_until_next_note => {
                        let (seconds, root, pitch) = (i.seconds, c.root_key, c.pitch_cents);
                        let (tempo, ppq) = (song.tempo, song.ppq);
                        sample_length = Some(Box::new(move |key| {
                            sample_ticks(seconds, key as f64 - root as f64 + pitch as f64 / 100.0, tempo, ppq)
                        }));
                    }
                    Some(_) => {}
                }
                devices.push(Device::Sampler {
                    sample: c.sample.clone(),
                    info,
                    root_key: c.root_key.min(127),
                    tune: c.pitch_cents as f64 / 100.0,
                });
                TrackKind::Sampler
            }
            Some(p) => {
                if let Some(plugin) = Self::plugin(p, Role::Instrument, &mut skipped) {
                    // FL plays a plugin's root key as C5, so a moved root key transposes it, as does the pitch knob.
                    let cents = (c.pitch_cents as f64 / 100.0).round_ties_even() as i32;
                    let shift = (DEFAULT_ROOT as i32 - c.root_key as i32).saturating_add(cents);
                    if shift != 0 {
                        devices.push(transpose(shift));
                    }
                    if c.pitch_cents % 100 != 0 {
                        self.notes.push(format!(
                            "{}: its pitch knob ({} semitones) is rounded to whole semitones",
                            c.name,
                            notes::signed(c.pitch_cents as f64 / 100.0)
                        ));
                    }
                    instrument = Some((devices.len(), p.clone()));
                    devices.push(plugin);
                }
                TrackKind::Instrument
            }
        };
        self.insert_effects(c, TrackRef::Track(at), &mut devices, &mut skipped);
        // When all its clips are on muted FL playlist tracks, the track is muted rather than its clips.
        let track_muted = clips::all_on_muted_tracks(&c.clips);
        let made = clips::of(c, clips::on_muted_tracks(&c.clips, track_muted), song.ppq)
            .into_iter()
            .map(|clip| FlClip {
                notes: clips::lengthen(clip.notes.clone(), clip.length, sample_length.as_deref()),
                ..clip
            })
            .collect();
        let on = c.clips.iter().filter(|x| x.on_muted_track).map(|x| x.track);
        if let Some(n) = notes::on_muted_tracks(&c.name, on, &song.muted_tracks, track_muted) {
            self.notes.push(n);
        }
        if !skipped.is_empty() {
            self.notes.push(format!("{}: left out {}", c.name, skipped.join(", ")));
        }
        if c.volume_envelope.is_some() {
            self.notes.push(format!("{}: {}", c.name, notes::ENVELOPE_LEFT_OUT));
        }
        let track = Track {
            name: c.name.clone(),
            source: c.name.clone(),
            channel: c.index,
            kind,
            devices,
            clips: made,
            audio_clips: Vec::new(),
            sample: None,
            gain: gain(c.volume),
            pan: pan(c.pan),
            muted: track_muted || !c.enabled,
        };
        (track, instrument)
    }

    /// An audio clip channel's clips in the playlist on audio tracks, stretched and pitched as in FL. Clips that
    /// overlap (on different playlist tracks in FL) play at once, so each goes on the first of its tracks that's free.
    fn audio_tracks(&mut self, c: &FlChannel, first: usize) -> Vec<Track> {
        let song = self.song;
        let mut skipped = Vec::new();
        let info = (self.read_sample)(&c.sample);
        let mut sorted: Vec<&FlAudioClip> = c.audio_clips.iter().collect();
        sorted.sort_by_key(|x| x.start);
        let mut lanes: Vec<Vec<FlAudioClip>> = Vec::new();
        for clip in sorted {
            let free =
                lanes.iter_mut().find(|l| l.last().is_some_and(|x| x.start.saturating_add(x.length) <= clip.start));
            match free {
                Some(lane) => lane.push(clip.clone()),
                None => lanes.push(vec![clip.clone()]),
            }
        }
        let mut made = Vec::new();
        for clips in &lanes {
            let mut devices = Vec::new();
            let mut lane_skipped = Vec::new();
            let skipped_here = if made.is_empty() { &mut skipped } else { &mut lane_skipped };
            self.insert_effects(c, TrackRef::Track(first + made.len()), &mut devices, skipped_here);
            let name = if made.is_empty() { c.name.clone() } else { format!("{} {}", c.name, made.len() + 1) };
            // When all its clips are on muted FL playlist tracks, the track is muted rather than its clips.
            let track_muted = clips::all_audio_on_muted_tracks(clips);
            let audio_clips = clips::audio_on_muted_tracks(clips, track_muted)
                .iter()
                .map(|x| audio_clip(c, x, info.as_ref(), song.tempo, song.ppq))
                .collect();
            let on = clips.iter().filter(|x| x.on_muted_track).map(|x| x.track);
            if let Some(n) = notes::on_muted_tracks(&name, on, &song.muted_tracks, track_muted) {
                self.notes.push(n);
            }
            made.push(Track {
                name,
                source: c.name.clone(),
                channel: c.index,
                kind: TrackKind::Audio,
                devices,
                clips: Vec::new(),
                audio_clips,
                sample: Some((c.sample.clone(), info.clone())),
                gain: gain(c.volume),
                pan: pan(c.pan),
                muted: track_muted || !c.enabled,
            });
        }
        if lanes.len() > 1 {
            self.notes.push(format!(
                "{}: its audio clips overlap in FL Studio, so they're on {} tracks",
                c.name,
                lanes.len()
            ));
        }
        match &info {
            None => self.notes.push(format!(
                "{}: couldn't read {}, so Bitwig will ask where it is, and how far its clips are stretched is a guess",
                c.name, c.sample
            )),
            Some(i) if i.seconds <= 0.0 => self.notes.push(format!(
                "{}: couldn't tell how long {} is, so Bitwig will ask where it is, and how far its clips are stretched is \
                 a guess",
                c.name, c.sample
            )),
            Some(_) => {}
        }
        if c.stretch.reversed {
            self.notes
                .push(format!("{}: plays its sample backwards in FL Studio; reverse its clips in Bitwig", c.name));
        }
        if c.pitch_cents != 0 {
            self.notes.push(format!(
                "{}: its channel pitch knob ({} semitones) isn't carried over to its audio clips; the pitch in its time \
                 stretching is",
                c.name,
                notes::signed(c.pitch_cents as f64 / 100.0)
            ));
        }
        if !skipped.is_empty() {
            self.notes.push(format!("{}: left out {}", c.name, skipped.join(", ")));
        }
        made
    }
}

/// One of an FL audio clip channel's clips: the clip's audio is the whole sample, stretched to play as long as it does
/// in FL and transposed by the semitones it's pitched in FL; the clip plays the part of it the FL clip does.
fn audio_clip(c: &FlChannel, fl: &FlAudioClip, info: Option<&SampleFile>, tempo: f64, ppq: u16) -> AudioClip {
    let beats = |ticks: i64| ticks as f64 / ppq.max(1) as f64;
    let length = beats(fl.length);
    let beats_per_second = tempo / 60.0;
    // How long the sample is: when it couldn't be read, a guess that the clip plays to its end.
    let seconds = match info {
        Some(i) if i.seconds > 0.0 => i.seconds,
        _ => fl.from + length / beats_per_second / played_seconds(&c.stretch, 1.0, 0.0).max(1e-6),
    };
    let played = played_seconds(&c.stretch, seconds, tempo) * beats_per_second;
    let offset = if seconds > 0.0 { fl.from / seconds * played } else { 0.0 };
    AudioClip {
        start: beats(fl.start),
        length,
        offset,
        played,
        seconds,
        semitones: stretched_semitones(&c.stretch, seconds, played / beats_per_second),
        muted: fl.muted,
    }
}

/// Adds an automation clip's points to the lane for what it moves on a track, making the lane if it's the first.
fn add_lane(lanes: &mut Vec<Lane>, track: TrackRef, target: Target, a: &FlAutomation, value: impl Fn(f64) -> f64) {
    let points =
        a.points.iter().map(|&p: &FlPoint| LanePoint { beat: p.beat, value: value(p.value), from: a.start_of(p) });
    match lanes.iter_mut().find(|l| l.track == track && l.target.same_as(&target)) {
        Some(lane) => {
            lane.points.extend(points);
            if !lane.clips.contains(&a.name) {
                lane.clips.push(a.name.clone());
            }
        }
        None => lanes.push(Lane { clips: vec![a.name.clone()], track, target, points: points.collect() }),
    }
}

/// What an FL song becomes. `read_sample` says what's known of a sample file (`None` when it can't be read).
pub fn from_fl(song: &FlSong, options: Options, read_sample: &mut dyn FnMut(&str) -> Option<SampleFile>) -> Plan {
    let inserts: HashMap<u32, &FlInsert> = song.inserts.iter().map(|i| (i.index, i)).collect();
    // Audio clip channels play their clips on audio tracks; one with no notes needs no instrument track too.
    let audio: HashSet<u32> =
        song.channels.iter().filter(|c| !c.audio_clips.is_empty() && !c.sample.is_empty()).map(|c| c.index).collect();
    // Every other channel with a plugin or sample gets an instrument track, as in FL's Channel Rack, even one the
    // playlist never plays, so its sound comes over.
    let playing: HashSet<u32> = song
        .channels
        .iter()
        .filter(|c| !c.notes.is_empty() || !c.clips.is_empty() || !audio.contains(&c.index))
        .filter(|c| c.instrument.is_some() || !c.sample.is_empty())
        .map(|c| c.index)
        .collect();
    let mut feeding: BTreeMap<u32, usize> = BTreeMap::new();
    for c in &song.channels {
        let carried = playing.contains(&c.index) || audio.contains(&c.index) && options.audio_tracks;
        if carried && c.insert > 0 {
            *feeding.entry(c.insert).or_default() += 1;
        }
    }
    let shared = feeding.into_iter().filter(|&(_, n)| n > 1).map(|(i, _)| i).collect();

    let mut planner = Planner {
        song,
        options,
        read_sample,
        inserts,
        shared,
        notes: Vec::new(),
        effect_copies: HashMap::new(),
        tracks_with_effects: HashMap::new(),
    };
    let mut tracks: Vec<Track> = Vec::new();
    // Each carried channel's tracks (its instrument track first), and its plugin's place on its instrument track.
    let mut placed: HashMap<u32, (usize, usize)> = HashMap::new();
    let mut instruments: HashMap<u32, (usize, FlPlugin)> = HashMap::new();

    for c in &song.channels {
        let first = tracks.len();
        let mut own = Vec::new();
        if playing.contains(&c.index) {
            let (track, instrument) = planner.instrument_track(c, first);
            if let Some(i) = instrument {
                instruments.insert(c.index, i);
            }
            own.push(track);
            if c.clips.is_empty() && !audio.contains(&c.index) {
                planner.notes.push(format!("{}: {}", c.name, notes::PLAYS_NOTHING));
            }
        }
        if !c.audio_clips.is_empty() && c.sample.is_empty() {
            planner.notes.push(format!(
                "{}: its audio clips are left out (FL Studio didn't save which sample they play)",
                c.name
            ));
        } else if audio.contains(&c.index) && !options.audio_tracks {
            planner.notes.push(format!(
                "{}: its audio clips are left out (the Bitwig template has no audio track with an audio clip on it that \
                 can be copied; it can't be in a group)",
                c.name
            ));
        } else if audio.contains(&c.index) {
            let at = first + own.len();
            own.extend(planner.audio_tracks(c, at));
        }
        if own.is_empty() {
            continue;
        }
        let copies = planner.tracks_with_effects.get(&c.index).copied().unwrap_or(0);
        if copies > 1
            && let Some(insert) = planner.inserts.get(&c.insert)
        {
            planner.notes.push(format!(
                "{}: each of its {copies} tracks has its own copy of {}'s effects, which FL Studio runs once on all of \
                 them",
                c.name,
                notes::insert_name(insert)
            ));
        }
        if !c.enabled {
            planner.notes.push(format!("{}: {}", c.name, notes::switched_off(own.len())));
        }
        if !c.switched_by.is_empty() {
            let audio_carried = audio.contains(&c.index) && options.audio_tracks;
            planner.notes.push(format!("{}: {}", c.name, notes::switched_on_and_off(c, audio_carried)));
        }
        placed.insert(c.index, (first, own.len()));
        tracks.extend(own);
    }

    // The master insert's effects on the master.
    let mut master_skipped = Vec::new();
    let master = planner.inserts.get(&0).copied();
    let master_effects = planner.effects(master, TrackRef::Master, 0, &mut master_skipped);
    if !master_skipped.is_empty() {
        planner.notes.push(format!("Master: left out {}", master_skipped.join(", ")));
    }

    // FL's insert faders and pans aren't carried yet (how FL's fader maps to gain isn't worked out).
    let mut used: Vec<u32> =
        placed.keys().filter_map(|i| song.channels.iter().find(|c| c.index == *i)).map(|c| c.insert).collect();
    used.push(0);
    used.sort_unstable();
    used.dedup();
    if let Some(moved) = notes::moved_inserts(used.iter().filter_map(|i| planner.inserts.get(i).copied())) {
        planner.notes.push(moved);
    }

    // Automation clips as lanes on the track they move: its volume, pan or a setting of its plugin, or a setting of an
    // effect on a mixer insert, on each track that has a copy of it.
    let mut lanes = Vec::new();
    for a in &song.automation {
        if a.control == FlControl::InsertEffect {
            let copies = match (a.insert, a.slot) {
                (Some(i), Some(s)) => planner.effect_copies.get(&(i, s)),
                _ => None,
            };
            let left = match copies {
                None => Some("its effect isn't converted"),
                Some(copies) => match copies.first().and_then(|(_, _, p)| setting_path(p, a.parameter)) {
                    None => Some("FL didn't save which of the effect's settings it moves"),
                    Some((path, id)) => {
                        for (track, device, effect) in copies {
                            let name = setting_name(&a.name, &effect.name);
                            let target = Target::Plugin { device: *device, path: path.clone(), id, name };
                            add_lane(&mut lanes, *track, target, a, |v| v);
                        }
                        None
                    }
                },
            };
            if let Some(left) = left {
                planner.notes.push(format!("{}: automation left out ({left})", a.name));
            }
            continue;
        }
        let channel = match a.channel {
            Some(ch) if !matches!(a.control, FlControl::Pitch | FlControl::Other) => ch,
            _ => {
                planner.notes.push(format!("{}: automation left out ({})", a.name, notes::not_carried(a, song)));
                continue;
            }
        };
        // Only channel controls are left here; a mixer or song control always comes without a channel.
        if !matches!(a.control, FlControl::Volume | FlControl::Pan | FlControl::PluginParameter | FlControl::OnOff) {
            planner.notes.push(format!("{}: automation left out ({})", a.name, notes::not_carried(a, song)));
            continue;
        }
        let Some(&(first, count)) = placed.get(&channel) else {
            planner.notes.push(format!("{}: automation left out (what it moves isn't converted)", a.name));
            continue;
        };
        match a.control {
            // The channel's volume or pan moves all its tracks' faders.
            FlControl::Volume => {
                for t in first..first + count {
                    add_lane(&mut lanes, TrackRef::Track(t), Target::Volume, a, gain);
                }
            }
            FlControl::Pan => {
                for t in first..first + count {
                    add_lane(&mut lanes, TrackRef::Track(t), Target::Pan, a, pan);
                }
            }
            // The plugin is on the channel's instrument track, its first.
            FlControl::PluginParameter => match instruments.get(&channel) {
                None => planner.notes.push(format!("{}: automation left out (its plugin isn't converted)", a.name)),
                Some((device, plugin)) => match setting_path(plugin, a.parameter) {
                    None => planner.notes.push(format!(
                        "{}: automation left out (FL didn't save which of the plugin's settings it moves)",
                        a.name
                    )),
                    Some((path, id)) => {
                        let channel_name = tracks.get(first).map(|t| t.name.as_str()).unwrap_or_default();
                        let name = setting_name(&a.name, channel_name);
                        add_lane(
                            &mut lanes,
                            TrackRef::Track(first),
                            Target::Plugin { device: *device, path, id, name },
                            a,
                            |v| v,
                        );
                    }
                },
            },
            // Carried by muting what the channel plays while it's off (see FlChannel::switched_by).
            _ => {}
        }
    }
    // Several FL clips moving one thing share a lane, in time order; where two meet on a beat, the one starting there
    // goes last, so it has the last word.
    for lane in &mut lanes {
        lane.points.sort_by(|x, y| {
            let beat = |p: &LanePoint| (p.beat * 1e4).round_ties_even();
            beat(x).total_cmp(&beat(y)).then(x.from.total_cmp(&y.from))
        });
    }

    let mut seen = HashSet::new();
    let notes = planner.notes.into_iter().filter(|n| seen.insert(n.clone())).collect();
    Plan { title: song.title.clone(), tempo: song.tempo, ppq: song.ppq, tracks, master_effects, lanes, notes }
}
