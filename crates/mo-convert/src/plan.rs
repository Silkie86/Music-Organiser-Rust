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

/// The master track, or a track by its place in [`Plan::tracks`], or the song itself (its tempo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrackRef {
    Master,
    Track(usize),
    Song,
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
    /// Switches a device on (1) and off (0): the device by its place on the track.
    OnOff { device: usize },
    /// The song's tempo in BPM (on [`TrackRef::Song`]).
    Tempo,
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
    /// The FL automation clip it comes from, by its place in [`Lane::clips`].
    pub source: usize,
}

/// An automation lane: every FL clip moving one thing on one track, in song order.
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    /// The FL automation clips it's made from, so a writer that can't carry it can say which.
    pub clips: Vec<String>,
    pub track: TrackRef,
    pub target: Target,
    pub points: Vec<LanePoint>,
    /// The FL mixer insert whose fader or pan it comes from; `None` when it's the track's own channel's.
    pub from_insert: Option<u32>,
    /// FL's edit events (knob moves recorded into a pattern) as automation clips on the lane, one for each place the
    /// pattern plays. Their moves are in [`Lane::points`] too, for a writer that can't make automation clips.
    pub parts: Vec<LanePart>,
}

/// An automation clip on a lane: where it starts and how long it is (beats from the song's start), its points (beats
/// from its start, values in the target's units), and the FL clip it comes from, by its place in [`Lane::clips`].
#[derive(Clone, Debug, PartialEq)]
pub struct LanePart {
    pub start: f64,
    pub length: f64,
    pub points: Vec<(f64, f64)>,
    pub source: usize,
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

/// The decibels FL Studio 2025 shows for a mixer insert's fader at 0.00, 0.01 ... 1.00 (its automation value), as
/// measured on Solomon's PC (2026-11-01, FL Studio's own reading, rounded to 0.001 dB). It's no simple curve, so the
/// table is used as it is.
const INSERT_FADER_DB: [f64; 101] = [
    f64::NEG_INFINITY,
    -50.335,
    -44.183,
    -40.529,
    -37.898,
    -35.827,
    -34.109,
    -32.636,
    -31.341,
    -30.182,
    -29.131,
    -28.166,
    -27.273,
    -26.439,
    -25.657,
    -24.918,
    -24.217,
    -23.549,
    -22.912,
    -22.3,
    -21.711,
    -21.144,
    -20.596,
    -20.065,
    -19.55,
    -19.05,
    -18.563,
    -18.087,
    -17.624,
    -17.17,
    -16.727,
    -16.292,
    -15.866,
    -15.448,
    -15.037,
    -14.633,
    -14.235,
    -13.844,
    -13.458,
    -13.078,
    -12.703,
    -12.333,
    -11.967,
    -11.605,
    -11.248,
    -10.894,
    -10.545,
    -10.198,
    -9.855,
    -9.516,
    -9.179,
    -8.845,
    -8.514,
    -8.186,
    -7.86,
    -7.536,
    -7.215,
    -6.896,
    -6.579,
    -6.264,
    -5.951,
    -5.64,
    -5.331,
    -5.023,
    -4.717,
    -4.413,
    -4.11,
    -3.809,
    -3.509,
    -3.21,
    -2.912,
    -2.616,
    -2.321,
    -2.028,
    -1.735,
    -1.443,
    -1.153,
    -0.863,
    -0.575,
    -0.287,
    0.0,
    0.286,
    0.571,
    0.855,
    1.139,
    1.422,
    1.704,
    1.985,
    2.266,
    2.546,
    2.826,
    3.104,
    3.383,
    3.66,
    3.938,
    4.214,
    4.49,
    4.766,
    5.041,
    5.316,
    5.59,
];

/// A mixer insert's fader (0 to 1, FL's automation value; 0.8 is 0 dB) as a linear gain, between the measured points.
pub fn insert_gain(fader: f64) -> f64 {
    // Past the table's end (only reached with no part left) counts as silence; -inf dB is a gain of 0 too.
    let gain = |i: usize| INSERT_FADER_DB.get(i).map_or(0.0, |db| 10f64.powf(db / 20.0));
    if fader.is_nan() {
        return 1.0;
    }
    let at = fader.clamp(0.0, 1.0) * 100.0;
    let below = at.floor() as usize;
    let part = at - below as f64;
    gain(below) + (gain(below + 1) - gain(below)) * part
}

/// The most a Bitwig fader goes up to: +6 dB.
const MAX_GAIN: f64 = 2.0;

/// How far an insert fader moves between two points before one is put in between: Bitwig draws a straight line on its
/// own fader scale, so in-between points keep a long fade on FL's curve.
const FADER_STEP: f64 = 0.05;

/// An insert fader clip's points with points put in between, where two points of one placed clip are further apart
/// than [`FADER_STEP`], along FL's straight line.
fn on_fader_curve(a: &FlAutomation) -> Vec<FlPoint> {
    let mut out: Vec<FlPoint> = Vec::with_capacity(a.points.len());
    for &q in &a.points {
        if let Some(&p) = out.last()
            && a.start_of(p) == a.start_of(q)
            && q.beat > p.beat + SAME_BEAT
        {
            let steps = ((q.value - p.value).abs() / FADER_STEP - 1e-9).ceil().min(100.0) as usize;
            for i in 1..steps {
                let part = i as f64 / steps as f64;
                out.push(FlPoint {
                    beat: p.beat + (q.beat - p.beat) * part,
                    value: p.value + (q.value - p.value) * part,
                });
            }
        }
        out.push(q);
    }
    out
}

/// FL's tempo automation value (0 to 1) in BPM: 10 to 522 in a straight line, as measured in FL Studio 2025.
pub fn tempo_bpm(value: f64) -> f64 {
    10.0 + 512.0 * value.clamp(0.0, 1.0)
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
    /// Each sample read once (rule 12), however many channels play it.
    samples: HashMap<String, Option<SampleFile>>,
    inserts: HashMap<u32, &'a FlInsert>,
    shared: HashSet<u32>,
    notes: Vec<String>,
    effect_copies: EffectCopies,
    /// How many of each channel's tracks have their own copy of its insert's effects.
    tracks_with_effects: HashMap<u32, usize>,
}

impl Planner<'_> {
    /// What's known of a sample, read once.
    fn sample(&mut self, path: &str) -> Option<SampleFile> {
        if let Some(known) = self.samples.get(path) {
            return known.clone();
        }
        let info = (self.read_sample)(path);
        self.samples.insert(path.to_owned(), info.clone());
        info
    }

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
                let info = self.sample(&c.sample);
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
            .map(|mut clip| {
                clip.notes = clips::lengthen(std::mem::take(&mut clip.notes), clip.length, sample_length.as_deref());
                clip
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
    /// `first` is where the first goes in the plan's tracks, and `taken` how many tracks the channel has already (its
    /// instrument track), so each of its tracks has its own name.
    fn audio_tracks(&mut self, c: &FlChannel, first: usize, taken: usize) -> Vec<Track> {
        let song = self.song;
        let mut skipped = Vec::new();
        let info = self.sample(&c.sample);
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
            let n = taken + made.len();
            let name = if n == 0 { c.name.clone() } else { format!("{} {}", c.name, n + 1) };
            // When all its clips are on muted FL playlist tracks, the track is muted rather than its clips.
            let track_muted = clips::all_on_muted_tracks(clips);
            let audio_clips = clips::on_muted_tracks(clips, track_muted)
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
    // How long the sample is. When it couldn't be read: stretched to a set length, a guess that it plays at its own
    // speed over that length; otherwise a guess that the clip plays to its end.
    let seconds = match info {
        Some(i) if i.seconds > 0.0 => i.seconds,
        _ if c.stretch.time > 0 && tempo > 0.0 => played_seconds(&c.stretch, 0.0, tempo),
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

/// The mixer inserts a channel's sound goes through from `start`: all of it (on through each insert sending only to
/// one other insert, a bus), and part of it (past an insert sending to several, or to the master and another insert:
/// a send, or a sidechain, which FL also marks there), the master left out.
fn sound_path(routes: &BTreeMap<u32, Vec<u32>>, start: u32) -> (Vec<u32>, HashSet<u32>) {
    let mut whole = vec![start];
    let mut at = start;
    let mut part = HashSet::new();
    loop {
        match routes.get(&at).map(Vec::as_slice) {
            Some(&[next]) if next != 0 && !whole.contains(&next) => {
                whole.push(next);
                at = next;
            }
            Some(to) if to.len() > 1 => {
                let mut todo = to.to_vec();
                while let Some(i) = todo.pop() {
                    if i != 0 && !whole.contains(&i) && part.insert(i) {
                        todo.extend(routes.get(&i).into_iter().flatten().copied());
                    }
                }
                break;
            }
            _ => break,
        }
    }
    (whole, part)
}

/// Adds an FL clip's points to the lane moving `target` on `track`, starting the lane when there's none; returns the
/// lane's place.
fn add_lane(
    lanes: &mut Vec<Lane>,
    track: TrackRef,
    target: Target,
    a: &FlAutomation,
    value: impl Fn(f64) -> f64,
) -> usize {
    let at = match lanes.iter().position(|l| l.track == track && l.target.same_as(&target)) {
        Some(at) => at,
        None => {
            lanes.push(Lane {
                clips: Vec::new(),
                track,
                target,
                points: Vec::new(),
                from_insert: None,
                parts: Vec::new(),
            });
            lanes.len() - 1
        }
    };
    let Some(lane) = lanes.get_mut(at) else { return at };
    lane.clips.push(a.name.clone());
    let source = lane.clips.len() - 1;
    lane.points.extend(a.points.iter().map(|&p: &FlPoint| LanePoint {
        beat: p.beat,
        value: value(p.value),
        from: a.start_of(p),
        source,
    }));
    lane.parts.extend(a.parts.iter().map(|part| LanePart {
        start: part.start,
        length: part.length,
        points: part.points.iter().map(|p| (p.beat, value(p.value))).collect(),
        source,
    }));
    at
}

/// How far apart two points can be and still count as on the same beat (as the FL reader counts them).
const SAME_BEAT: f64 = 1e-4;

/// A lane's points in time order. Where two FL clips meet on a beat, the one starting there goes last, so it has the
/// last word; where one clip's points stop and another's start later, the value holds until then and jumps, as FL
/// keeps a setting where the last clip left it (a straight line would slide it).
fn in_song_order(mut points: Vec<LanePoint>) -> Vec<LanePoint> {
    points.sort_by(|x, y| {
        let beat = |p: &LanePoint| (p.beat * 1e4).round_ties_even();
        beat(x).total_cmp(&beat(y)).then(x.from.total_cmp(&y.from))
    });
    let mut out: Vec<LanePoint> = Vec::with_capacity(points.len());
    for q in points {
        if let Some(&p) = out.last()
            && p.source != q.source
            && q.beat > p.beat + SAME_BEAT
        {
            out.push(LanePoint { beat: q.beat, ..p });
        }
        out.push(q);
    }
    out
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
        samples: HashMap::new(),
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
            let taken = own.len();
            own.extend(planner.audio_tracks(c, at, taken));
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

    // Effects on inserts no carried channel feeds (a bus fed by sends or by other inserts) aren't carried yet.
    let fed: HashSet<u32> =
        placed.keys().filter_map(|i| song.channels.iter().find(|c| c.index == *i)).map(|c| c.insert).collect();
    for insert in &song.inserts {
        let third_party: Vec<&str> =
            insert.effects.iter().filter(|e| matches!(e.format, "VST2" | "VST3")).map(|e| e.name.as_str()).collect();
        if insert.index != 0 && !fed.contains(&insert.index) && !third_party.is_empty() {
            planner.notes.push(format!(
                "{}: its effects ({}) are left out, as no converted channel feeds it directly (sends and routing \
                 between inserts aren't carried over yet)",
                notes::insert_label(insert),
                third_party.join(", ")
            ));
        }
    }

    // FL's insert faders and pans where they sit (not automated) aren't carried yet: the fader's curve is measured as
    // an automation value, but not yet what the saved fader value is on that scale.
    // A channel's sound goes through its insert and on through the buses it's routed to, all of it.
    let paths: HashMap<u32, (Vec<u32>, HashSet<u32>)> = song
        .channels
        .iter()
        .filter(|c| placed.contains_key(&c.index))
        .map(|c| (c.index, sound_path(&song.routes, c.insert)))
        .collect();
    let mut used: Vec<u32> = paths.values().flat_map(|(whole, _)| whole.iter().copied()).collect();
    used.push(0);
    used.sort_unstable();
    used.dedup();
    if let Some(moved) = notes::moved_inserts(used.iter().filter_map(|i| planner.inserts.get(i).copied())) {
        planner.notes.push(moved);
    }
    // Knob moves recorded into patterns (FL's edit events) that move anything but a plugin's setting aren't carried
    // yet; said, so they don't vanish unseen.
    if let Some(note) = notes::edit_events(&song.edit_events) {
        planner.notes.push(note);
    }

    // Automation clips as lanes on the track they move: its volume, pan or a setting of its plugin, or a setting of an
    // effect on a mixer insert or the effect's slot switched on and off, on each track that has a copy of it.
    let mut lanes = Vec::new();
    let mut faders = Vec::new();
    for a in &song.automation {
        if matches!(a.control, FlControl::InsertVolume | FlControl::InsertPan) && a.insert.is_some() {
            faders.push(a);
            continue;
        }
        if matches!(a.control, FlControl::InsertSlotOnOff | FlControl::InsertEffect) {
            let copies = match (a.insert, a.slot) {
                (Some(i), Some(s)) => planner.effect_copies.get(&(i, s)),
                _ => None,
            };
            let left = match copies {
                None => Some("its effect isn't converted"),
                // FL counts 0.5 and above as on.
                Some(copies) if a.control == FlControl::InsertSlotOnOff => {
                    for (track, device, _) in copies {
                        let on = |v: f64| if v >= 0.5 { 1.0 } else { 0.0 };
                        add_lane(&mut lanes, *track, Target::OnOff { device: *device }, a, on);
                    }
                    None
                }
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
        // The song's tempo, on Bitwig's tempo lane.
        if a.control == FlControl::Tempo {
            add_lane(&mut lanes, TrackRef::Song, Target::Tempo, a, tempo_bpm);
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
    // A mixer insert's fader and pan on the tracks of the channels feeding it (the master insert's on the master),
    // after the channels' own volume and pan: Bitwig has one fader for both, so the lane is the channel's level times
    // the insert's, and an insert's pan is carried only where the channel's pan is in the middle. Where a channel's
    // own volume or pan is automated too, that lane is kept and the insert's is left out.
    for &a in &faders {
        let Some(insert) = a.insert else { continue };
        let pan_lane = a.control == FlControl::InsertPan;
        let target = if pan_lane { Target::Pan } else { Target::Volume };
        let mut left = Vec::new();
        let onto: Vec<TrackRef> = if insert == 0 {
            vec![TrackRef::Master]
        } else {
            let mut onto = Vec::new();
            for c in &song.channels {
                let (Some(&(first, count)), Some((whole, part))) = (placed.get(&c.index), paths.get(&c.index)) else {
                    continue;
                };
                let name = tracks.get(first).map_or("", |t| t.name.as_str());
                // Bitwig has one fader for the track: it follows the automated insert nearest the channel.
                let nearest = whole
                    .iter()
                    .find(|&&i| faders.iter().any(|f| f.insert == Some(i) && f.control == a.control))
                    .copied();
                if nearest == Some(insert) {
                    onto.extend((first..first + count).map(TrackRef::Track));
                } else if let Some(nearer) = nearest.filter(|_| whole.contains(&insert)) {
                    let what = if pan_lane { "pan" } else { "fader" };
                    left.push(format!("{name} follows insert {nearer}'s {what}, nearer to it"));
                } else if part.contains(&insert) {
                    left.push(format!("{name} reaches it through a send or sidechain"));
                }
            }
            onto
        };
        if onto.is_empty() {
            let why = if left.is_empty() { "no converted channel feeds it".to_owned() } else { left.join("; ") };
            planner.notes.push(format!("{}: automation left out ({why})", a.name));
            continue;
        }
        let mut capped = Vec::new();
        let dense = FlAutomation { points: on_fader_curve(a), ..a.clone() };
        let mut added = false;
        for track in onto {
            let own = match track {
                TrackRef::Master | TrackRef::Song => None,
                TrackRef::Track(t) => tracks.get(t),
            };
            let automated = lanes.iter().any(|l| l.track == track && l.target == target && l.from_insert.is_none());
            if automated {
                left.push(format!(
                    "{} has its own {} automated",
                    own.map_or("the master", |t| t.name.as_str()),
                    if pan_lane { "pan" } else { "volume" }
                ));
                continue;
            }
            if pan_lane && own.is_some_and(|t| t.pan.abs() > 1e-9) {
                left.push(format!("{}'s own pan isn't in the middle", own.map_or("", |t| t.name.as_str())));
                continue;
            }
            added = true;
            let at = if pan_lane {
                add_lane(&mut lanes, track, Target::Pan, a, pan)
            } else {
                // The master's lane is FL's master fader alone: Bitwig's lane sets the master's fader.
                let level = own.map_or(1.0, |t| t.gain);
                if dense.points.iter().any(|p| level * insert_gain(p.value) > MAX_GAIN + 1e-9) {
                    capped.push(own.map_or("the master", |t| t.name.as_str()).to_owned());
                }
                add_lane(&mut lanes, track, Target::Volume, &dense, |v| (level * insert_gain(v)).min(MAX_GAIN))
            };
            if let Some(lane) = lanes.get_mut(at) {
                lane.from_insert = Some(insert);
            }
        }
        if !left.is_empty() {
            let some = if added { " on some tracks" } else { "" };
            planner.notes.push(format!("{}: automation left out{some} ({})", a.name, left.join("; ")));
        }
        if !capped.is_empty() {
            planner.notes.push(format!(
                "{}: on {} the fader and the channel's own level come to more than Bitwig's +6 dB, so the loudest part \
                 stays at +6 dB",
                a.name,
                capped.join(", ")
            ));
        }
    }
    // Several FL clips moving one thing share a lane.
    for lane in &mut lanes {
        lane.points = in_song_order(std::mem::take(&mut lane.points));
        // Automation clips in song order, as Bitwig keeps them (it shows a lane's clips only up to one that starts
        // before the one ahead of it: Solomon's sly_17, 2026-11-02). Where a clip starts before the one ahead of it
        // ends (two patterns placed over each other), it takes over from there, as the curve does.
        lane.parts.sort_by(|a, b| a.start.total_cmp(&b.start));
        for k in 1..lane.parts.len() {
            let next = lane.parts.get(k).map_or(f64::INFINITY, |p| p.start);
            if let Some(part) = lane.parts.get_mut(k - 1)
                && part.start + part.length > next + SAME_BEAT
            {
                part.length = next - part.start;
                let length = part.length;
                part.points.retain(|p| p.0 < length - SAME_BEAT);
            }
        }
        lane.parts.retain(|p| !p.points.is_empty() && p.length > SAME_BEAT);
        // Until FL's first tempo point the song plays at the project's tempo; a lane would hold its first point.
        if lane.target == Target::Tempo
            && let Some(&first) = lane.points.first()
            && first.beat > SAME_BEAT
        {
            let held = LanePoint { beat: 0.0, value: song.tempo, from: 0.0, ..first };
            lane.points.splice(0..0, [held, LanePoint { beat: first.beat, ..held }]);
        }
    }

    let mut seen = HashSet::new();
    let notes = planner.notes.into_iter().filter(|n| seen.insert(n.clone())).collect();
    Plan { title: song.title.clone(), tempo: song.tempo, ppq: song.ppq, tracks, master_effects, lanes, notes }
}
