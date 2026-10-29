//! FL Studio `.flp` projects.
//!
//! The format isn't published, but it's well worked out (the C# app's reader, PyFLP, and Solomon's saves from FL 4
//! to FL 26). See [`events`] for how the file is laid out.

pub mod events;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime, TimeDelta};

use crate::model::{Confidence, PluginKind, PluginRef, ProjectInfo};
use crate::plugins;
use events::*;

pub const DAW: &str = "FL Studio";
/// Goes up by one whenever the reader finds more (or different) things in the same files, so a scan reads every FL
/// project again (rule 7). The C# reader is at 7; 8 is what the Rust reader finds differently (DIFFERENCES.md).
pub const VERSION: u32 = 8;
pub const EXTENSIONS: [&str; 1] = ["flp"];

/// The Fruity Wrapper (FL's VST host) keeps a VST's details as records: u32 version, then (u32 id, u64 size, data).
const VST_STATE: u32 = 53;
const VST_NAME: u32 = 54;
const VST_PATH: u32 = 55;
const VST_VENDOR: u32 = 56;

const CHANNEL_TYPE_INSTRUMENT: u32 = 2;
const WRAPPER: &str = "Fruity Wrapper";

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

fn is_native_generator(name: &str) -> bool {
    NATIVE_GENERATORS.iter().any(|g| g.eq_ignore_ascii_case(name))
}

/// Reads what's in an FL Studio project. A file that isn't one is listed with what's wrong with it, not an error;
/// only a file that can't be read from disk is. Samples saved under FL's user data folder are found from where the
/// project lives.
pub fn read(path: &Path) -> std::io::Result<ProjectInfo> {
    let raw = std::fs::read(path)?;
    let mut info = read_bytes(&raw);
    resolve_user_data(&mut info.samples, path);
    Ok(info.dedupe())
}

/// [`read`] for a file already in memory. It never looks at the disk, so samples under FL's user data folder stay as
/// FL wrote them (`%FLStudioUserData%\...`); [`resolve_user_data`] finds them.
pub fn read_bytes(raw: &[u8]) -> ProjectInfo {
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
    let mut channel: Option<u32> = None;
    let mut in_mixer = false;
    let mut pending_internal = String::new();
    let mut channel_has_plugin = false;
    let (mut patterns, mut notes) = (0, 0);
    // Plugins that saved nothing, by name; letter case aside, each once, in order.
    let mut nothing_saved: BTreeMap<String, String> = BTreeMap::new();
    let is_instrument_channel = |channel: Option<u32>, in_mixer: bool, types: &BTreeMap<u32, u32>| {
        !in_mixer && channel.is_some_and(|c| types.get(&c) == Some(&CHANNEL_TYPE_INSTRUMENT))
    };

    for e in &flp.events {
        match e.id {
            EV_TEMPO => info.tempo = Some(e.int() as f64 / 1000.0),
            EV_TEMPO_COARSE if info.tempo.is_none() => info.tempo = Some(e.int() as f64),
            EV_TITLE => info.title = flp.text(&e.data),
            EV_GENRE => _ = info.details.insert("Genre".into(), flp.text(&e.data)),
            EV_ARTISTS => _ = info.details.insert("Artists".into(), flp.text(&e.data)),
            EV_CHANNEL_NEW => {
                channel = Some(e.int());
                in_mixer = false;
                channel_has_plugin = false;
            }
            EV_CHANNEL_TYPE => {
                if let Some(c) = channel {
                    channel_types.insert(c, e.int());
                }
            }
            EV_PATTERN_NEW => patterns += 1,
            EV_MIXER_SLOT | EV_INSERT_FLAGS => in_mixer = true,
            EV_SAMPLE_PATH => info.samples.push(flp.text(&e.data)),
            EV_PATTERN_NOTES => notes += e.data.len() / note_size(&e.data, flp.major, e.offset.is_none()),
            EV_PROJECT_TIME if e.data.len() >= 16 => {
                let (created, spent) = project_time(&e.data);
                info.created = created;
                if let Some(spent) = spent {
                    info.details.insert("Time spent".into(), spent);
                }
            }
            EV_PLUGIN_NAME | EV_CHANNEL_NAME_OLD if !in_mixer => {
                if let Some(c) = channel {
                    channel_names.entry(c).or_insert_with(|| flp.text(&e.data));
                }
            }
            EV_PLUGIN_INTERNAL => {
                pending_internal = flp.text(&e.data);
                // A channel hosts one generator; a second plugin before the next channel means the mixer's effect
                // slots have started.
                if channel_has_plugin {
                    in_mixer = true;
                }
                channel_has_plugin = true;
                if !pending_internal.is_empty() && !pending_internal.eq_ignore_ascii_case(WRAPPER) {
                    let instrument = is_native_generator(&pending_internal)
                        || is_instrument_channel(channel, in_mixer, &channel_types);
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
                let f = wrapper_fields(&e.data);
                let name = wrapper_text(&f, VST_NAME);
                let vst_path = wrapper_text(&f, VST_PATH);
                if name.is_empty() && vst_path.is_empty() {
                    pending_internal.clear();
                    continue;
                }
                let shown = if name.is_empty() { plugins::name_from_path(&vst_path) } else { name };
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
                let instrument = is_instrument_channel(channel, in_mixer, &channel_types);
                info.plugins.push(PluginRef {
                    name: shown,
                    vendor: if own && vendor.is_empty() { "Image-Line".into() } else { vendor },
                    kind: if instrument { PluginKind::Instrument } else { PluginKind::Effect },
                    format: format.into(),
                    path: vst_path,
                    id: String::new(),
                });
                pending_internal.clear();
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
    info.dedupe()
}

/// The size of the note records in an event, by FL's version (rule 9): 24 bytes from FL 9, 20 bytes in FL 4 to 8.
/// Notes the reader made from old step sequencer dots are always 24. Without a version, or in an FL 4 to 8 save whose
/// notes don't divide into 20 bytes, the size is told from the notes themselves.
fn note_size(data: &[u8], major: u32, made_here: bool) -> usize {
    match major {
        _ if made_here => 24,
        9.. => 24,
        1.. if data.len().is_multiple_of(20) => 20,
        _ => size_from_notes(data),
    }
}

/// The record size for notes from a file with no FL version: the size that reads them in time order (as FL keeps
/// them), else the one that divides them, 24 when both or neither do.
fn size_from_notes(data: &[u8]) -> usize {
    let in_order = |size: usize| {
        !data.is_empty()
            && data.len().is_multiple_of(size)
            && data.chunks_exact(size).map(|r| u32::from_le_bytes([r[0], r[1], r[2], r[3]])).is_sorted()
    };
    if in_order(24) {
        24
    } else if in_order(20) || (data.len().is_multiple_of(20) && !data.len().is_multiple_of(24)) {
        20
    } else {
        24
    }
}

/// The Fruity Wrapper's records by id, each a slice of the event rather than a copy (a plugin's state can be
/// megabytes). A record that runs past the end stops the reading; a later record with the same id wins.
fn wrapper_fields(blob: &[u8]) -> BTreeMap<u32, &[u8]> {
    let mut fields = BTreeMap::new();
    if blob.len() < 4 {
        return fields;
    }
    let version = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
    if version == 0 || version > 64 {
        return fields;
    }
    let mut pos = 4;
    while pos + 12 <= blob.len() {
        let id = u32::from_le_bytes(blob[pos..pos + 4].try_into().expect("4 bytes"));
        let size = u64::from_le_bytes(blob[pos + 4..pos + 12].try_into().expect("8 bytes"));
        pos += 12;
        if size > (blob.len() - pos) as u64 {
            break;
        }
        fields.insert(id, &blob[pos..pos + size as usize]);
        pos += size as usize;
    }
    fields
}

fn wrapper_text(fields: &BTreeMap<u32, &[u8]>, id: u32) -> String {
    fields.get(&id).map_or_else(String::new, |b| String::from_utf8_lossy(b).trim_end_matches('\0').to_owned())
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
fn saved_nothing(state: &[u8]) -> bool {
    matches!(vst2_header(state), Some((_, 0, _))) && state[4] & 0x08 != 0
}

/// Event 237: the day the project was first made (a Delphi date: days since 30 Dec 1899) and how long FL Studio has
/// had it open, in days. Saving again, even in a newer FL, keeps the date. Dates out of range are left out.
pub fn project_time(data: &[u8]) -> (Option<NaiveDateTime>, Option<String>) {
    if data.len() < 16 {
        return (None, None);
    }
    let made = f64::from_le_bytes(data[..8].try_into().expect("8 bytes"));
    let open = f64::from_le_bytes(data[8..16].try_into().expect("8 bytes"));
    // 1995 to 2119.
    let created = (made.is_finite() && made > 35_000.0 && made < 80_000.0).then(|| {
        let start = NaiveDate::from_ymd_opt(1899, 12, 30).expect("a real date").and_hms_opt(0, 0, 0).expect("midnight");
        start + TimeDelta::milliseconds((made * 86_400_000.0).round() as i64)
    });
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

/// FL writes paths under its user data folder as `%FLStudioUserData%\...`, the folder that holds the Projects folder.
/// It's found from where the project lives: `I:\FL Studio Data\FL Studio\Projects\Song.flp` gives
/// `I:\FL Studio Data\FL Studio`. Samples no folder above the project fits stay as FL wrote them. Each folder name
/// is looked for once, however many samples share it.
pub fn resolve_user_data(samples: &mut [String], project: &Path) {
    let full = std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf());
    // The first folder (or the file, for a sample right in the user data folder) by name: the folder above the
    // project that has it, if any.
    let mut found: HashMap<(bool, String), Option<PathBuf>> = HashMap::new();
    for sample in samples.iter_mut() {
        let Some(rest) = after_user_data(sample) else { continue };
        let parts: Vec<&str> = rest.split(['\\', '/']).collect();
        let nested = parts.len() > 1;
        let dir = found.entry((nested, parts[0].to_lowercase())).or_insert_with(|| {
            full.ancestors().skip(1).filter(|d| !d.as_os_str().is_empty()).find_map(|dir| {
                let there = dir.join(parts[0]);
                let fits = if nested { there.is_dir() } else { there.is_file() };
                fits.then(|| dir.to_path_buf())
            })
        });
        if let Some(dir) = dir {
            *sample = parts.iter().fold(dir.clone(), |p, part| p.join(part)).to_string_lossy().into_owned();
        }
    }
}

/// What follows `%FLStudioUserData%` (letter case aside) and its slash, for a path that starts with it.
fn after_user_data(sample: &str) -> Option<&str> {
    const USER_DATA: &str = "%FLStudioUserData%";
    plugins::starts_with_ignore_case(sample, USER_DATA)
        .then(|| sample[USER_DATA.len()..].trim_start_matches(['\\', '/']))
}
