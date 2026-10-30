//! FL Studio `.flp` projects.
//!
//! The format isn't published, but it's well worked out (the C# app's reader, PyFLP, and Solomon's saves from FL 4
//! to FL 26). See [`events`] for how the file is laid out.

pub mod automation;
pub mod events;
mod notes;
pub mod presets;
pub mod song;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime, TimeDelta};

use crate::model::{Confidence, PluginKind, PluginRef, ProjectInfo, SampleUse};
use crate::plugins;
use events::*;

pub const DAW: &str = "FL Studio";
/// Goes up by one whenever the reader finds more (or different) things in the same files, so a scan reads every FL
/// project again (rule 7). The C# reader is at 7; 8 is what the Rust reader finds differently (DIFFERENCES.md); 9 adds
/// the samples a song holds but doesn't play.
pub const VERSION: u32 = 9;
pub const EXTENSIONS: [&str; 1] = ["flp"];

/// The Fruity Wrapper (FL's VST host) keeps a VST's details as records: u32 version, then (u32 id, u64 size, data).
pub(crate) const VST_KIND: u32 = 50;
pub(crate) const VST_FOUR_CC: u32 = 51;
pub(crate) const VST_CLASS_ID: u32 = 52;
pub(crate) const VST_STATE: u32 = 53;
pub(crate) const VST_NAME: u32 = 54;
pub(crate) const VST_PATH: u32 = 55;
pub(crate) const VST_VENDOR: u32 = 56;
/// The wrapper's kind (record 50) for a VST3.
pub(crate) const KIND_VST3: u8 = 8;

pub(crate) const CHANNEL_TYPE_INSTRUMENT: u32 = 2;
pub(crate) const WRAPPER: &str = "Fruity Wrapper";

/// FL's own instruments: a native plugin with one of these names is an instrument wherever it sits.
const NATIVE_GENERATORS: &[&str] = &[
    "3x Osc",
    "FL Keys",
    "FPC",
    "Sytrus",
    "Harmor",
    "Harmless",
    "FLEX",
    "Fruity DX10",
    "Transistor Bass",
    "GMS",
    "Sakura",
    "PoiZone",
    "Toxic Biohazard",
    "Morphine",
    "Ogun",
    "Kepler",
    "Kepler Exo",
    "Slicex",
    "Fruity Slicer",
    "DirectWave",
    "BooBass",
    "Drumaxx",
    "Drumpad",
    "Autogun",
    "MiniSynth",
    "Plucked!",
    "BeepMap",
    "Fruity Granulizer",
    "Sawer",
    "SimSynth",
    "Wasp",
    "Wasp XT",
    "Groove Machine Synth",
    "Vital",
    "Transporter",
    "PyroSyn",
    "Fruity DrumSynth Live",
    "Fruity Envelope Controller",
    "Fruity Keyboard Controller",
    "Sampler",
    "Audio Clip",
    "Layer",
    "MIDI Out",
    "Fruit Kick",
    "Fruity Kick",
];

pub(crate) fn is_native_generator(name: &str) -> bool {
    NATIVE_GENERATORS.iter().any(|g| g.eq_ignore_ascii_case(name))
}

/// Reads what's in an FL Studio project. A file that isn't one is listed with what's wrong with it, not an error;
/// only a file that can't be read from disk is. Samples saved under FL's user data folder are found from where the
/// project lives.
pub fn read(path: &Path) -> std::io::Result<ProjectInfo> {
    Ok(read_from(&std::fs::read(path)?, path))
}

/// [`read`] for a file already in memory that lives at `path`: samples under FL's user data folder are found from
/// there, as [`read`] finds them.
pub fn read_from(raw: &[u8], path: &Path) -> ProjectInfo {
    let mut user_data = UserData::new(path);
    read_with(raw, &mut |s| user_data.resolve(s))
}

/// [`read`] for a file already in memory. It never looks at the disk, so samples under FL's user data folder stay as
/// FL wrote them (`%FLStudioUserData%\...`); [`UserData`] finds them.
pub fn read_bytes(raw: &[u8]) -> ProjectInfo {
    read_with(raw, &mut |s| s.to_owned())
}

/// Reads the project, with `resolve` giving each sample's path. Samples are resolved before anything is said about
/// them, so one file written two ways counts once, and plays if either channel plays it.
fn read_with(raw: &[u8], resolve: &mut dyn FnMut(&str) -> String) -> ProjectInfo {
    let mut info = ProjectInfo::new(DAW);
    let flp = match events::load(raw) {
        Ok(flp) => flp,
        Err(Damaged) => {
            info.confidence = Confidence::Catalogue;
            info.warnings.push(format!("{DAMAGED}."));
            return info;
        }
    };
    info.version = flp.version.clone();
    info.confidence = Confidence::Full;
    info.can_extract_midi = true;
    info.can_extract_presets = true;
    if let Some(w) = flp.warning {
        info.warnings.push(w.to_owned());
    }
    info.details.insert("PPQ".into(), flp.ppq.to_string());

    let mut channel_names: BTreeMap<u32, String> = BTreeMap::new();
    let mut channel_types: BTreeMap<u32, u32> = BTreeMap::new();
    let mut rack = Rack::default();
    let mut pending_internal = String::new();
    let (mut patterns, mut notes) = (0, 0);
    let known = channels(&flp);
    // Plugins that saved nothing, by name; letter case aside, each once, in order.
    let mut nothing_saved: BTreeMap<String, String> = BTreeMap::new();
    let instrument_channel = |rack: &Rack, types: &BTreeMap<u32, u32>| {
        rack.channel().is_some_and(|c| types.get(&c) == Some(&CHANNEL_TYPE_INSTRUMENT))
    };

    for e in &flp.events {
        rack.step(e);
        match e.id {
            EV_TEMPO => info.tempo = Some(e.int() as f64 / 1000.0),
            EV_TEMPO_COARSE if info.tempo.is_none() => info.tempo = Some(e.int() as f64),
            EV_TITLE => info.title = flp.text(&e.data),
            EV_GENRE => _ = info.details.insert("Genre".into(), flp.text(&e.data)),
            EV_ARTISTS => _ = info.details.insert("Artists".into(), flp.text(&e.data)),
            EV_CHANNEL_TYPE => {
                if let Some(c) = rack.channel() {
                    channel_types.insert(c, e.int());
                }
            }
            EV_PATTERN_NEW => patterns += 1,
            EV_SAMPLE_PATH => info.samples.push(resolve(&flp.text(&e.data))),
            EV_PATTERN_NOTES => {
                notes += e.data.len() / notes::note_size(&e.data, flp.major, e.offset.is_none(), Some(&known))
            }
            EV_PROJECT_TIME if e.data.len() >= 16 => {
                let (created, spent) = project_time(&e.data);
                info.created = created;
                if let Some(spent) = spent {
                    info.details.insert("Time spent".into(), spent);
                }
            }
            EV_PLUGIN_NAME | EV_CHANNEL_NAME_OLD => {
                if let Some(c) = rack.channel() {
                    channel_names.entry(c).or_insert_with(|| flp.text(&e.data));
                }
            }
            EV_PLUGIN_INTERNAL => {
                pending_internal = flp.text(&e.data);
                if !pending_internal.is_empty() && !pending_internal.eq_ignore_ascii_case(WRAPPER) {
                    let instrument =
                        is_native_generator(&pending_internal) || instrument_channel(&rack, &channel_types);
                    info.plugins.push(PluginRef {
                        name: pending_internal.clone(),
                        vendor: "Image-Line".into(),
                        kind: if instrument { PluginKind::Instrument } else { PluginKind::Effect },
                        format: "Native".into(),
                        ..PluginRef::default()
                    });
                }
            }
            EV_PLUGIN_STATE if pending_internal.eq_ignore_ascii_case(WRAPPER) => {
                pending_internal.clear();
                let f = wrapper_fields(&e.data);
                let shown = wrapper_plugin_name(&f);
                if shown.is_empty() {
                    continue;
                }
                let vst_path = wrapper_text(&f, VST_PATH);
                if f.get(&VST_STATE).is_some_and(|s| saved_nothing(s)) {
                    nothing_saved.entry(shown.to_uppercase()).or_insert_with(|| shown.clone());
                }
                let vendor = wrapper_text(&f, VST_VENDOR);
                // Old FL (FL 9) hosted its own effects, such as Fruity Compressor, in the wrapper: they're still FL's own.
                let own = plugins::is_fl_studio_bundled(&shown, &vendor, &vst_path);
                let format = match plugins::format_from_path(&vst_path) {
                    _ if own => "Native",
                    "" => "VST",
                    known => known,
                };
                let instrument = instrument_channel(&rack, &channel_types);
                info.plugins.push(PluginRef {
                    name: shown,
                    vendor: if own && vendor.is_empty() { "Image-Line".into() } else { vendor },
                    kind: if instrument { PluginKind::Instrument } else { PluginKind::Effect },
                    format: format.into(),
                    path: vst_path,
                    id: String::new(),
                });
            }
            _ => {}
        }
    }

    let numbers: BTreeSet<u32> = channel_names.keys().chain(channel_types.keys()).copied().collect();
    for i in numbers {
        info.tracks.push(channel_names.get(&i).cloned().unwrap_or_else(|| format!("Channel {}", i + 1)));
    }
    for name in nothing_saved.values() {
        info.warnings.push(format!(
            "{name} has no saved sound in this file (it gave FL Studio nothing when the project was saved), so it opens \
             at its default sound. An earlier save of the project may still have it; converting to Bitwig takes it from \
             there."
        ));
    }
    info.details.insert("Channels".into(), info.tracks.len().to_string());
    info.details.insert("Patterns".into(), patterns.to_string());
    info.details.insert("Notes".into(), notes.to_string());
    // Only a song with samples has any to leave unplayed.
    if !info.samples.is_empty() {
        note_unplayed(&song::read_song_with(&flp, false), resolve, &mut info);
    }
    align_unplayed(info.dedupe())
}

/// Follows which channel the events belong to. A channel's events start at event 64; the mixer's at an insert (236),
/// a slot's end (98), or a second plugin on one channel (a channel hosts one generator, so a second plugin before the
/// next channel is the mixer's first effect). Every reader walks the events with one, so they all agree.
///
/// It also counts the mixer's inserts: each starts at its flags (event 236), from 0 (the master), and its name comes
/// just before them.
#[derive(Default)]
pub(crate) struct Rack {
    channel: Option<u32>,
    in_mixer: bool,
    has_plugin: bool,
    insert: Option<u32>,
    next_insert_name: Option<Vec<u8>>,
    insert_name: Option<Vec<u8>>,
}

impl Rack {
    /// Takes in the next event.
    pub fn step(&mut self, e: &Event<'_>) {
        match e.id {
            EV_CHANNEL_NEW => {
                self.channel = Some(e.int());
                self.in_mixer = false;
                self.has_plugin = false;
            }
            EV_MIXER_SLOT => self.in_mixer = true,
            EV_INSERT_FLAGS => {
                self.in_mixer = true;
                self.insert = Some(self.insert.map_or(0, |i| i + 1));
                self.insert_name = self.next_insert_name.take().filter(|n| !n.is_empty());
            }
            EV_INSERT_NAME => self.next_insert_name = Some(e.data.to_vec()),
            EV_PLUGIN_INTERNAL => {
                if self.has_plugin {
                    self.in_mixer = true;
                }
                self.has_plugin = true;
            }
            _ => {}
        }
    }

    /// The channel the event belongs to: none for the mixer's events, or before the first channel.
    pub fn channel(&self) -> Option<u32> {
        self.channel.filter(|_| !self.in_mixer)
    }

    /// The mixer's events have started (and no channel's since).
    pub fn in_mixer(&self) -> bool {
        self.in_mixer
    }

    /// The mixer insert whose events these are, once the first has started.
    pub fn insert(&self) -> Option<u32> {
        self.insert
    }

    /// At an insert's flags: the name it was given (as saved), if any.
    pub fn insert_name(&self, e: &Event<'_>) -> Option<&[u8]> {
        self.insert_name.as_deref().filter(|_| e.id == EV_INSERT_FLAGS)
    }
}

/// The song's channels, by their number in the Channel Rack.
pub(crate) fn channels(flp: &Flp<'_>) -> HashSet<u32> {
    flp.events.iter().filter(|e| e.id == EV_CHANNEL_NEW).map(Event::int).collect()
}

/// Keys `unplayed` by the spelling `samples` has (letter case can differ between a channel's events), dropping any
/// that isn't in `samples`.
fn align_unplayed(mut info: ProjectInfo) -> ProjectInfo {
    let spelling: HashMap<String, &String> = info.samples.iter().map(|s| (s.to_lowercase(), s)).collect();
    info.unplayed = std::mem::take(&mut info.unplayed)
        .into_iter()
        .filter_map(|(s, u)| spelling.get(&s.to_lowercase()).map(|&s| (s.clone(), u)))
        .collect();
    info
}

/// The samples this save holds but doesn't play, and why: on a channel with nothing in the playlist, or muted (the
/// channel switched off, or every clip of it muted or on a muted playlist track). A sample on several channels plays
/// when any of them does. Letter case aside, each sample once, under the first channel's spelling.
fn note_unplayed(song: &song::FlSong, resolve: &mut dyn FnMut(&str) -> String, info: &mut ProjectInfo) {
    let mut order: Vec<String> = Vec::new();
    let mut by_sample: HashMap<String, (String, Vec<&song::FlChannel>)> = HashMap::new();
    for c in song.channels.iter().filter(|c| !c.sample.is_empty()) {
        let sample = resolve(&c.sample);
        let key = sample.to_lowercase();
        if !by_sample.contains_key(&key) {
            order.push(key.clone());
        }
        by_sample.entry(key).or_insert_with(|| (sample, Vec::new())).1.push(c);
    }
    for key in order {
        let (sample, channels) = &by_sample[&key];
        let plays = |c: &&song::FlChannel| {
            c.enabled && (!c.notes.is_empty() || c.audio_clips.iter().any(|a| !a.muted && !a.on_muted_track))
        };
        let placed =
            |c: &&song::FlChannel| c.clips.iter().any(|clip| !clip.notes.is_empty()) || !c.audio_clips.is_empty();
        if !channels.iter().any(plays) {
            let why = if channels.iter().any(placed) { SampleUse::Muted } else { SampleUse::NoNotes };
            info.unplayed.insert(sample.clone(), why);
        }
    }
}

/// The Fruity Wrapper's records by id, each a slice of the event rather than a copy (a plugin's state can be
/// megabytes). A record that runs past the end stops the reading; a later record with the same id wins.
pub(crate) fn wrapper_fields(blob: &[u8]) -> BTreeMap<u32, &[u8]> {
    if blob.len() < 4 || !(1..=64).contains(&u32_at(blob, 0)) {
        return BTreeMap::new();
    }
    records(blob).0
}

/// Records after a u32 version: (u32 id, u64 size, data), by id, a later one with the same id winning. Says whether
/// they all fit; a record that runs past the end stops the reading.
pub(crate) fn records(blob: &[u8]) -> (BTreeMap<u32, &[u8]>, bool) {
    let mut records = BTreeMap::new();
    let mut pos = 4;
    while pos + 12 <= blob.len() {
        let id = u32_at(blob, pos);
        let size = u64_at(blob, pos + 4).unwrap_or(u64::MAX);
        pos += 12;
        if size > (blob.len() - pos) as u64 {
            return (records, false);
        }
        records.insert(id, &blob[pos..pos + size as usize]);
        pos += size as usize;
    }
    (records, true)
}

pub(crate) fn wrapper_text(fields: &BTreeMap<u32, &[u8]>, id: u32) -> String {
    fields.get(&id).map_or_else(String::new, |b| String::from_utf8_lossy(b).trim_end_matches('\0').to_owned())
}

/// The plugin's name: the one the wrapper keeps, else its file's name ("" when it has neither).
pub(crate) fn wrapper_plugin_name(fields: &BTreeMap<u32, &[u8]>) -> String {
    match wrapper_text(fields, VST_NAME) {
        n if n.is_empty() => plugins::name_from_path(&wrapper_text(fields, VST_PATH)),
        n => n,
    }
}

/// A VST3, by the wrapper's kind or its file's ending.
pub(crate) fn wrapper_is_vst3(fields: &BTreeMap<u32, &[u8]>) -> bool {
    fields.get(&VST_KIND).and_then(|k| k.first()) == Some(&KIND_VST3)
        || wrapper_text(fields, VST_PATH).to_lowercase().ends_with(".vst3")
}

/// A VST2's four letters, as a number.
pub(crate) fn wrapper_four_cc(fields: &BTreeMap<u32, &[u8]>) -> Option<i32> {
    fields.get(&VST_FOUR_CC).and_then(|c| <[u8; 4]>::try_from(*c).ok()).map(i32::from_le_bytes)
}

/// A VST3's class id.
pub(crate) fn wrapper_class_id<'a>(fields: &BTreeMap<u32, &'a [u8]>) -> Option<&'a [u8; 16]> {
    fields.get(&VST_CLASS_ID).and_then(|c| <&[u8; 16]>::try_from(*c).ok())
}

/// Where the plugin's own state starts after the header FL puts in front of a VST2's state, its length, and the
/// plugin's program that was selected (FL selects it again after loading the state).
///
/// Newer FL (seen in FL 21 and 25): i32 -9, u8 flags (0x08 = the plugin saves a chunk), i32 -2, u32 chunk length,
/// u32 0, u32 program, so 21 bytes. FL 9 (wrapper version 6) has no -2 or 0: i32 -9, u8 flags, u32 chunk length,
/// u32 program, so 13 bytes.
pub(crate) fn vst2_header(state: &[u8]) -> Option<(usize, u32, u32)> {
    let i32_at = |at| events::i32_at(state, at);
    if state.len() < 13 || i32_at(0) != -9 {
        return None;
    }
    if i32_at(5) == -2 {
        return (state.len() >= 21).then(|| (21, i32_at(9) as u32, i32_at(17).max(0) as u32));
    }
    Some((13, i32_at(5) as u32, i32_at(9).max(0) as u32))
}

/// A VST2 that saves its sound as a chunk but gave FL an empty one, so FL kept none of it.
pub(crate) fn saved_nothing(state: &[u8]) -> bool {
    matches!(vst2_header(state), Some((_, 0, _))) && state[4] & 0x08 != 0
}

/// Event 237: the day the project was first made (a Delphi date: days since 30 Dec 1899) and how long FL Studio has
/// had it open, in days. Saving again, even in a newer FL, keeps the date. Dates out of range are left out.
pub fn project_time(data: &[u8]) -> (Option<NaiveDateTime>, Option<String>) {
    let (Some(made), Some(open)) = (f64_at(data, 0), f64_at(data, 8)) else { return (None, None) };
    let created = NaiveDate::from_ymd_opt(1899, 12, 30)
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        // 1995 to 2119.
        .filter(|_| made.is_finite() && made > 35_000.0 && made < 80_000.0)
        .map(|start| start + TimeDelta::milliseconds((made * 86_400_000.0).round() as i64));
    let spent = (open.is_finite() && open > 0.0 && open < 3650.0).then(|| {
        // To the millisecond first, so 13 minutes saved as 0.009027... days isn't 12.99999 minutes.
        let minutes = (open * 86_400_000.0).round() as i64 / 60_000;
        if minutes >= 60 {
            format!("{} h {} min", minutes / 60, minutes % 60)
        } else {
            format!("{} min", minutes.max(1))
        }
    });
    (created, spent)
}

/// FL writes paths under its user data folder as `%FLStudioUserData%\...`, the folder that holds the Projects
/// folder. It's found from where the project lives: `I:\FL Studio Data\FL Studio\Projects\Song.flp` gives
/// `I:\FL Studio Data\FL Studio`. Samples no folder above the project fits stay as FL wrote them.
///
/// Finds samples under FL's user data folder for one project, looking for each folder name once however many samples
/// share it.
pub struct UserData {
    project: PathBuf,
    /// The first folder (or the file, for a sample right in the user data folder) by name: the folder above the
    /// project that has it, if any.
    found: HashMap<(bool, String), Option<PathBuf>>,
}

impl UserData {
    pub fn new(project: &Path) -> UserData {
        let project = std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf());
        UserData { project, found: HashMap::new() }
    }

    /// The sample's full path, or the path as FL wrote it when it isn't under the user data folder or no folder fits.
    pub fn resolve(&mut self, sample: &str) -> String {
        let Some(rest) = after_user_data(sample) else { return sample.to_owned() };
        let parts: Vec<&str> = rest.split(['\\', '/']).collect();
        let nested = parts.len() > 1;
        let project = &self.project;
        let dir = self.found.entry((nested, parts[0].to_lowercase())).or_insert_with(|| {
            project.ancestors().skip(1).filter(|d| !d.as_os_str().is_empty()).find_map(|dir| {
                let there = dir.join(parts[0]);
                let fits = if nested { there.is_dir() } else { there.is_file() };
                fits.then(|| dir.to_path_buf())
            })
        });
        match dir {
            Some(dir) => parts.iter().fold(dir.clone(), |p, part| p.join(part)).to_string_lossy().into_owned(),
            None => sample.to_owned(),
        }
    }
}

/// What follows `%FLStudioUserData%` (letter case aside) and its slash, for a path that starts with it.
fn after_user_data(sample: &str) -> Option<&str> {
    const USER_DATA: &str = "%FLStudioUserData%";
    plugins::starts_with_ignore_case(sample, USER_DATA)
        .then(|| sample[USER_DATA.len()..].trim_start_matches(['\\', '/']))
}
