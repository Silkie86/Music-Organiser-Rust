//! What's in a Bitwig Studio `.bwproject`: devices, plugins, tracks, tempo, note clips and the plugin states stored in
//! its zip, from the decoded file (see [`super::format`]). The field and class ids were mapped from projects saved by
//! Bitwig 5.1 to 6.1. When a file can't be decoded (a newer format, or damage), the reader looks for readable text in
//! it instead, as the C# app's first Bitwig reader did.
//!
//! A Bitwig project folder usually looks like `My Song/My Song.bwproject`, with `samples/`, `recordings/` and
//! `bounce/` beside it, so the audio in those folders belongs to the project too.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use super::format::{Document, ObjId, Value, decode};
use crate::model::{Confidence, MidiClip, NoteEvent, PluginKind, PluginRef, ProjectInfo};
use crate::paths;
use crate::plugins;
use crate::presets::PresetBlob;
use crate::textscan;

pub const DAW: &str = "Bitwig Studio";
/// Goes up by one whenever the reader finds more (or different) things in the same files, so a scan reads every
/// Bitwig project again (rule 7). 2 is the C# reader's: Bitwig 6.1's Sampler analysis decoded, and a collected sample
/// counted once.
pub const VERSION: u32 = 2;
pub const EXTENSIONS: [&str; 1] = ["bwproject"];

// Classes.
const CLS_BITWIG_DEVICE: u32 = 0x40;
const CLS_VST3_PLUGIN: u32 = 0x728;
const CLS_VST2_PLUGIN: u32 = 0x1a5;
const CLS_TEMPO: u32 = 0x4c1;
const CLS_CLIP: u32 = 0x47;
const CLS_KEY_LANE: u32 = 0x42;
const CLS_NOTE: u32 = 0x66;
const CLS_FILE_REF: u32 = 0x2a;
const CLS_HARDWARE_IO: u32 = 0x461;
// Fields.
const F_DEVICE_NAME: u32 = 0x9a;
const F_DEVICE_VENDOR: u32 = 0x9b;
const F_DEVICE_CATEGORY: u32 = 0x9c;
const F_DEVICE_PRESET: u32 = 0x12de;
const F_PLUGIN_STATE: u32 = 0xbe5;
const F_TEMPO: u32 = 0x2c8;
const F_TRACK_NAME: u32 = 0x15b;
const F_START: u32 = 0x2af;
const F_DURATION: u32 = 0x26;
const F_VELOCITY: u32 = 0xef;
const F_PITCH: u32 = 0xee;
const F_NOTES: u32 = 0x21f;
const F_FILE_PATH: u32 = 0x3b;
const F_HARDWARE_NAME: u32 = 0x11a7;
// The track tree: the root's project group holds tracks (and groups of tracks) in one list, effect tracks in another
// and the master in a third field. The "structure" metadata mirrors it with the names Bitwig shows.
const F_PROJECT_GROUP: u32 = 0x4dd;
const F_GROUP_TRACKS: u32 = 0x4de;
const F_GROUP_EFFECT_TRACKS: u32 = 0x4df;
const F_MASTER_TRACK: u32 = 0x4e0;
const F_STRUCT_ROOT: u32 = 0xad2;
const F_STRUCT_TRACKS: u32 = 0xace;
const F_STRUCT_EFFECT_TRACKS: u32 = 0xacf;
const F_STRUCT_NAME: u32 = 0xad1;

/// Bitwig's note clips count in ticks of this many to a beat.
const PPQ: u16 = 960;

const INSTRUMENTS: &[&str] = &[
    "Polymer",
    "Phase-4",
    "Polysynth",
    "FM-4",
    "Sampler",
    "E-Kick",
    "E-Snare",
    "E-Hat",
    "E-Clap",
    "E-Cowbell",
    "E-Tom",
    "Drum Machine",
    "Organ",
    "Poly Grid",
    "Wavetable",
    "Instrument Layer",
    "Instrument Selector",
    "HW Instrument",
];

const EFFECTS: &[&str] = &[
    "EQ+",
    "EQ-2",
    "EQ-5",
    "EQ-DJ",
    "Filter+",
    "Delay+",
    "Delay-1",
    "Delay-2",
    "Delay-4",
    "Reverb",
    "Convolution",
    "Compressor",
    "Compressor+",
    "Dynamics",
    "Transient Control",
    "Peak Limiter",
    "Chorus",
    "Chorus+",
    "Flanger",
    "Flanger+",
    "Phaser",
    "Phaser+",
    "Distortion",
    "Saturator",
    "Bit-8",
    "Amp",
    "Treemonster",
    "Tool",
    "Spectral Split",
    "Multiband FX-2",
    "Multiband FX-3",
    "FX Grid",
    "FX Layer",
    "FX Selector",
    "Freq Shifter",
    "Pitch Shifter",
    "Resonator Bank",
    "Ladder",
    "Sweep",
    "Fuzz",
    "Over",
    "Heat",
    "Rotary",
    "Mid-Side Split",
    "Gate",
    "Expander",
    "De-Esser",
    "Ring-Mod",
    "Comb",
    "Tape",
    "Pitch Echo",
    "Frequency Shifter+",
];

const NOTE_EFFECTS: &[&str] = &[
    "Arpeggiator",
    "Note Echo",
    "Note Latch",
    "Multi-Note",
    "Note Length",
    "Quantize",
    "Note Harmonizer",
    "Ricochet",
    "Key Filter",
    "Note Filter",
    "Velocity Curve",
    "Transposition Map",
    "Micro-pitch",
    "Note Grid",
];

/// Device categories that make a plugin an instrument (letter case aside).
const INSTRUMENT_CATEGORIES: &[&str] =
    &["Synth", "Instrument", "Drum Kit", "Drum", "Drums", "Sampler", "Organ", "Piano", "Hardware"];

fn is_instrument_category(category: &str) -> bool {
    INSTRUMENT_CATEGORIES.iter().any(|c| c.eq_ignore_ascii_case(category))
}

/// A decoded project, with its tracks' names worked out once for everything read from it.
pub struct Project {
    pub doc: Document,
    names: TrackNames,
}

impl Project {
    pub fn new(doc: Document) -> Project {
        let names = track_names(&doc);
        Project { doc, names }
    }

    /// The file's bytes, a project or not: decoded when it's Bitwig's (moved, not copied).
    pub fn open(raw: Vec<u8>) -> Result<Project, Vec<u8>> {
        if !super::is_bitwig(&raw) {
            return Err(raw);
        }
        Ok(Project::new(decode(raw).map_err(|_| Vec::new())?))
    }

    /// The note clips: see [`extract_midi`].
    pub fn midi(&self) -> Vec<MidiClip> {
        extract_midi(&self.doc, &self.names)
    }

    /// The plugin states: see [`extract_presets`].
    pub fn presets(&self) -> Vec<PresetBlob> {
        extract_presets(&self.doc, &self.names)
    }
}

/// Reads what's in a Bitwig project. A file that isn't one is listed with what's wrong with it, not an error; only a
/// file that can't be read from disk is. The file is held in memory once.
pub fn read(path: &Path) -> std::io::Result<ProjectInfo> {
    let info = match Project::open(std::fs::read(path)?) {
        Ok(project) => read_project(Some(&project), &project.doc.raw, path, &plugins::is_always_known),
        Err(raw) => read_project(None, &raw, path, &plugins::is_always_known),
    };
    Ok(info)
}

/// [`read`] for a file already in memory that lives at `path` (the folders beside it hold its audio).
pub fn read_from(raw: &[u8], path: &Path) -> ProjectInfo {
    let project = decode(raw).ok().map(Project::new);
    read_project(project.as_ref(), raw, path, &plugins::is_always_known)
}

/// [`read_from`] for a project already decoded (`None` when the file isn't Bitwig's); `raw` is the file's bytes.
/// `known` says whether a name is a plugin installed on this PC: only the text scan needs it, as it can't tell a
/// plugin's name from any other text otherwise.
pub fn read_project(project: Option<&Project>, raw: &[u8], path: &Path, known: &dyn Fn(&str) -> bool) -> ProjectInfo {
    let doc = project.map(|p| &p.doc);
    let mut info = ProjectInfo::new(DAW);
    let dir = path.parent().unwrap_or(Path::new("."));
    let used_files = doc.map(|d| d.meta_strings("used_files")).unwrap_or_default();
    let collected = || used_files.iter().flat_map(|f| collected_names(f)).map(|n| n.to_lowercase()).collect();

    match project {
        Some(p) if p.doc.root.is_some() && p.doc.body_error.is_none() => read_decoded(p, &collected(), &mut info),
        _ => {
            let collected = doc.map(|_| collected());
            text_scan(raw, dir, &mut info, doc.and_then(|d| d.body_error.as_deref()), collected.as_ref(), known)
        }
    }

    if let Some(d) = doc {
        if !d.meta_str("application_version_name").is_empty() {
            info.version = d.meta_str("application_version_name").to_owned();
        }
        if !d.meta_str("title").is_empty() {
            info.title = d.meta_str("title").to_owned();
        }
        for f in &used_files {
            if textscan::is_audio_path(f) || plugins::ends_with_ignore_case(f, ".multisample") {
                info.samples.push(paths::full_path(dir, f));
            }
        }
    }

    add_project_folder_audio(dir, &mut info);
    info.dedupe()
}

/// The object a field holds (following a reference), as the C# reader's `Obj`: not a named one, as
/// [`Document::field_obj`] also gives.
fn field_obj(doc: &Document, of: ObjId, id: u32) -> Option<ObjId> {
    match doc.obj(of).get(id)? {
        Value::Obj(s) | Value::BareRef(s) => doc.resolve(*s),
        _ => None,
    }
}

fn ids(doc: &Document) -> impl Iterator<Item = ObjId> + '_ {
    (0..doc.objects.len() as u32).map(ObjId)
}

fn read_decoded(project: &Project, collected: &HashSet<String>, info: &mut ProjectInfo) {
    let doc = &project.doc;
    info.confidence = Confidence::Partial;

    let file_refs = || ids(doc).filter(|&o| doc.obj(o).class == CLS_FILE_REF).map(|o| doc.obj(o).str(F_FILE_PATH));
    // Each plugin file with the plugin's name from it.
    let plugin_paths: Vec<(String, &str)> =
        file_refs().filter(|p| textscan::is_plugin_path(p)).map(|p| (plugins::name_from_path(p), p)).collect();

    for o in &doc.objects {
        if o.class == CLS_TEMPO
            && info.tempo.is_none()
            && let Some(bpm) = o.num(F_TEMPO)
            && (10.0..=999.0).contains(&bpm)
        {
            info.tempo = Some(bpm);
        }

        if !o.has(F_DEVICE_NAME) || !o.has(F_DEVICE_VENDOR) {
            continue;
        }
        let (name, vendor) = (o.str(F_DEVICE_NAME), o.str(F_DEVICE_VENDOR));
        if name.is_empty() {
            continue;
        }
        if o.class == CLS_BITWIG_DEVICE {
            let kind = bitwig_kind(name, o.str(F_DEVICE_CATEGORY));
            info.plugins.push(plugin(name, "Bitwig", kind, "Native", ""));
        } else if matches!(o.class, CLS_VST3_PLUGIN | CLS_VST2_PLUGIN) || o.has(F_PLUGIN_STATE) {
            let file = plugin_paths.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map_or("", |(_, p)| *p);
            let format = match o.class {
                CLS_VST3_PLUGIN => "VST3",
                CLS_VST2_PLUGIN => "VST2",
                _ => plugins::format_from_path(file),
            };
            let kind = if is_instrument_category(o.str(F_DEVICE_CATEGORY)) {
                PluginKind::Instrument
            } else {
                PluginKind::Effect
            };
            info.plugins.push(plugin(name, vendor, kind, format, file));
        }
    }

    let names = &project.names;
    info.tracks.extend(names.names.iter().filter(|(t, _)| Some(*t) != names.master).map(|(_, n)| n.clone()));

    let mut hardware: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for o in doc.objects.iter().filter(|o| o.class == CLS_HARDWARE_IO) {
        let name = o.str(F_HARDWARE_NAME);
        if !name.is_empty() && seen.insert(name.to_lowercase()) {
            hardware.push(name.to_owned());
        }
    }
    hardware.sort_by(|a, b| culture_order(a, b));
    if !hardware.is_empty() {
        info.details.insert("Hardware I/O".into(), hardware.join(", "));
    }
    info.hardware.extend(hardware);

    // Audio referenced by its full path. Bitwig also keeps where every file it collected into the project folder came
    // from; the copies are listed in `used_files`, so an original only counts when the project has no copy of it.
    for f in file_refs() {
        if !f.is_empty()
            && textscan::is_audio_path(f)
            && paths::is_absolute(f)
            && !collected.contains(&paths::file_name(f).to_lowercase())
        {
            info.samples.push(f.to_owned());
        }
    }

    let notes = doc.objects.iter().filter(|o| o.class == CLS_NOTE).count();
    info.details.insert("Notes".into(), notes.to_string());
    info.can_extract_midi = notes > 0;

    let states = zip_entries(doc).map_or(0, |z| z.names.iter().filter(|n| n.starts_with(STATES)).count());
    if states > 0 {
        info.details.insert("Stored plugin states".into(), states.to_string());
    }
    info.can_extract_presets = states > 0;
}

fn plugin(name: &str, vendor: &str, kind: PluginKind, format: &str, path: &str) -> PluginRef {
    PluginRef {
        name: name.to_owned(),
        vendor: vendor.to_owned(),
        kind,
        format: format.to_owned(),
        path: path.to_owned(),
        id: String::new(),
    }
}

/// Text in about the order .NET's default comparer puts it (as the C# app sorts hardware names): spaces and
/// punctuation before digits, digits before letters, and letters whatever their case. (Names differing only in case
/// are listed once.)
fn culture_order(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |s: &str| -> Vec<(u8, char)> {
        s.chars()
            .flat_map(char::to_lowercase)
            .map(|c| match c {
                c if c.is_alphabetic() => (2, c),
                c if c.is_numeric() => (1, c),
                c => (0, c),
            })
            .collect()
    };
    key(a).cmp(&key(b)).then_with(|| b.cmp(a))
}

/// The names a file collected into the project can have had before: its own, and its name without each "-2" style
/// ending Bitwig adds when the name is taken ("Snare.wav" collected as "Snare-2.wav"). Only the copy's endings are
/// taken off, so collecting "Kick-01.wav" doesn't hide an original "Kick-02.wav".
fn collected_names(copy: &str) -> Vec<String> {
    let name = paths::file_name(copy);
    let ext = paths::extension(name);
    let mut stem = &name[..name.len() - ext.len()];
    let mut out = vec![name.to_owned()];
    while let Some(dash) = stem.rfind('-')
        && dash + 1 < stem.len()
        && stem[dash + 1..].bytes().all(|b| b.is_ascii_digit())
    {
        stem = &stem[..dash];
        out.push(format!("{stem}{ext}"));
    }
    out
}

fn bitwig_kind(name: &str, category: &str) -> PluginKind {
    if INSTRUMENTS.contains(&name) || is_instrument_category(category) {
        PluginKind::Instrument
    } else if NOTE_EFFECTS.contains(&name) || category.eq_ignore_ascii_case("Note FX") {
        PluginKind::NoteEffect
    } else {
        PluginKind::Effect
    }
}

/// The tracks in a group's list and every list inside them, in order, each once.
fn walk(start: Vec<ObjId>, inside: impl Fn(ObjId) -> Vec<ObjId>) -> Vec<ObjId> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    // What's left of each list being walked, last first.
    let mut stack = vec![start.into_iter().rev().collect::<Vec<_>>()];
    while let Some(left) = stack.last_mut() {
        let Some(t) = left.pop() else {
            stack.pop();
            continue;
        };
        // A group listed inside itself would go round for ever.
        if seen.insert(t) {
            out.push(t);
            stack.push(inside(t).into_iter().rev().collect());
        }
    }
    out
}

struct TrackTree<T> {
    tracks: Vec<T>,
    effect_tracks: Vec<T>,
}

fn body_tracks(doc: &Document) -> (TrackTree<ObjId>, Option<ObjId>) {
    let Some(project) = doc.root.and_then(|r| field_obj(doc, r, F_PROJECT_GROUP)) else {
        return (TrackTree { tracks: Vec::new(), effect_tracks: Vec::new() }, None);
    };
    let tracks = walk(doc.field_list(project, F_GROUP_TRACKS), |g| doc.field_list(g, F_GROUP_TRACKS));
    let tree = TrackTree { tracks, effect_tracks: doc.field_list(project, F_GROUP_EFFECT_TRACKS) };
    (tree, field_obj(doc, project, F_MASTER_TRACK))
}

/// The names Bitwig shows for the tracks, from the "structure" summary in the metadata.
fn structure_names(doc: &Document) -> Option<TrackTree<String>> {
    let Some(Value::Blob(blob)) = doc.meta("structure") else { return None };
    let s = decode(blob.bytes()).ok()?;
    let project = s.root.and_then(|r| field_obj(&s, r, F_STRUCT_ROOT))?;
    if s.body_error.is_some() {
        return None;
    }
    let name = |t: ObjId| s.obj(t).str(F_STRUCT_NAME).to_owned();
    let tracks = walk(s.field_list(project, F_STRUCT_TRACKS), |g| s.field_list(g, F_STRUCT_TRACKS));
    Some(TrackTree {
        tracks: tracks.into_iter().map(name).collect(),
        effect_tracks: s.field_list(project, F_STRUCT_EFFECT_TRACKS).into_iter().map(name).collect(),
    })
}

/// Names for tracks, groups, effect tracks and the master, in the order they're named.
pub struct TrackNames {
    names: Vec<(ObjId, String)>,
    at: HashMap<ObjId, usize>,
    master: Option<ObjId>,
}

impl TrackNames {
    /// Names a track, or renames it in its place when it's named already.
    fn set(&mut self, t: ObjId, name: String) {
        match self.at.get(&t) {
            Some(&i) => {
                if let Some((_, named)) = self.names.get_mut(i) {
                    *named = name;
                }
            }
            None => {
                self.at.insert(t, self.names.len());
                self.names.push((t, name));
            }
        }
    }

    /// The name of the track `o` is on, or "Project".
    fn of(&self, doc: &Document, o: ObjId) -> &str {
        doc.ancestor(o, |p, _| self.at.contains_key(&p))
            .and_then(|t| self.names.get(*self.at.get(&t)?))
            .map_or("Project", |(_, name)| name)
    }
}

/// Display names for tracks, groups, effect tracks and the master. Uses Bitwig's own names when the structure
/// metadata lines up; otherwise the track's name, else (as Bitwig does) its first device's preset or the device itself.
pub fn track_names(doc: &Document) -> TrackNames {
    let (body, master) = body_tracks(doc);
    let shown = structure_names(doc);
    let all: HashSet<ObjId> = body.tracks.iter().chain(&body.effect_tracks).chain(&master).copied().collect();

    let mut device_names: HashMap<ObjId, &str> = HashMap::new();
    for o in ids(doc) {
        let object = doc.obj(o);
        if !object.has(F_DEVICE_NAME) || !matches!(object.class, CLS_BITWIG_DEVICE | CLS_VST3_PLUGIN | CLS_VST2_PLUGIN)
        {
            continue;
        }
        let Some(track) = doc.ancestor(o, |p, _| all.contains(&p)) else { continue };
        if device_names.contains_key(&track) {
            continue;
        }
        let preset = object.str(F_DEVICE_PRESET);
        let name = if preset.is_empty() { object.str(F_DEVICE_NAME) } else { preset };
        if !name.is_empty() {
            device_names.insert(track, name);
        }
    }

    let mut names = TrackNames { names: Vec::new(), at: HashMap::new(), master };
    let mut name = |tracks: &[ObjId], display: Option<&Vec<String>>, fallback: &str| {
        let display = display.filter(|d| d.len() == tracks.len());
        for (i, &t) in tracks.iter().enumerate() {
            let own = doc.obj(t).str(F_TRACK_NAME);
            let name = match display.and_then(|d| d.get(i)).map(String::as_str) {
                Some(shown) if !shown.is_empty() => shown.to_owned(),
                _ if !own.is_empty() => own.to_owned(),
                _ => match device_names.get(&t) {
                    Some(device) => (*device).to_owned(),
                    None => format!("{fallback} {}", i + 1),
                },
            };
            names.set(t, name);
        }
    };
    name(&body.tracks, shown.as_ref().map(|s| &s.tracks), "Track");
    name(&body.effect_tracks, shown.as_ref().map(|s| &s.effect_tracks), "FX");
    if let Some(m) = master {
        names.set(m, "Master".into());
    }
    names
}

/// One MIDI clip per Bitwig note clip, named after its track ("Bass 1", "Bass 2"...). Nothing for a file that didn't
/// decode cleanly.
pub fn extract_midi(doc: &Document, names: &TrackNames) -> Vec<MidiClip> {
    if doc.root.is_none() || doc.body_error.is_some() {
        return Vec::new();
    }
    // Each clip's track and notes, in the order the clips are first reached.
    let mut clips: Vec<(String, Vec<NoteEvent>)> = Vec::new();
    let mut clip_at: HashMap<ObjId, usize> = HashMap::new();
    for lane in ids(doc).filter(|&o| doc.obj(o).class == CLS_KEY_LANE) {
        let Some(pitch) = doc.obj(lane).num(F_PITCH) else { continue };
        let clip = doc.ancestor(lane, |_, a| a.class == CLS_CLIP).or(doc.obj(lane).parent.map(|(p, _)| p));
        let Some(clip) = clip else { continue };
        let at = *clip_at.entry(clip).or_insert_with(|| {
            clips.push((names.of(doc, lane).to_owned(), Vec::new()));
            clips.len() - 1
        });
        let Some((_, notes)) = clips.get_mut(at) else { continue };
        for note in doc.field_list(lane, F_NOTES) {
            let n = doc.obj(note);
            if n.class != CLS_NOTE {
                continue;
            }
            let start = n.num(F_START).unwrap_or(0.0);
            let length = n.num(F_DURATION).unwrap_or(0.25);
            let velocity = n.num(F_VELOCITY).unwrap_or(0.8);
            let ppq = PPQ as f64;
            notes.push(NoteEvent {
                start: (start.max(0.0) * ppq).round_ties_even() as i64,
                length: ((length * ppq).round_ties_even() as i64).max(1),
                key: (pitch as i32).clamp(0, 127) as u8,
                velocity: crate::model::velocity::from_unit(velocity),
                muted: false,
                step: false,
            });
        }
    }

    let mut out = Vec::new();
    let mut per_track: HashMap<String, u32> = HashMap::new();
    for (track, mut notes) in clips {
        if notes.is_empty() {
            continue;
        }
        let n = per_track.entry(track.to_lowercase()).or_insert(0);
        *n += 1;
        let mut clip = MidiClip { name: format!("{track} {n}"), ppq: PPQ, ..MidiClip::default() };
        notes.sort_by_key(|x| (x.start, x.key));
        clip.track(&track).extend(notes);
        out.push(clip);
    }
    out
}

/// Where Bitwig keeps plugin states in a project's zip.
const STATES: &str = "plugin-states/";

/// A project's zip, opened, with its files' names in the order the zip lists them.
struct Zip<'a> {
    archive: zip::ZipArchive<std::io::Cursor<&'a [u8]>>,
    names: Vec<String>,
}

fn zip_entries(doc: &Document) -> Option<Zip<'_>> {
    let archive = zip::ZipArchive::new(std::io::Cursor::new(doc.zip()?)).ok()?;
    let mut names = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        names.push(archive.name_for_index(i).and_then(Result::ok).map(|n| n.into_owned()).unwrap_or_default());
    }
    Some(Zip { archive, names })
}

/// The plugin states in the project's zip. Bitwig stores them as ordinary preset files (.vstpreset for a VST3, .fxb
/// or .fxp for a VST2), so they load straight into the plugin. Each is named after its track and its preset (or its
/// plugin). Read even from a file whose objects didn't all decode, as far as they did.
pub fn extract_presets(doc: &Document, names: &TrackNames) -> Vec<PresetBlob> {
    let Some(mut zip) = zip_entries(doc) else { return Vec::new() };
    // The device each state belongs to, by its file's name (letter case aside): the first that names it.
    let mut owners: HashMap<String, ObjId> = HashMap::new();
    for o in ids(doc) {
        let state = doc.obj(o).str(F_PLUGIN_STATE);
        if !state.is_empty() {
            owners.entry(state.to_lowercase()).or_insert(o);
        }
    }

    let mut presets = Vec::new();
    for (i, full) in zip.names.iter().enumerate() {
        if !full.starts_with(STATES) {
            continue;
        }
        let Ok(mut entry) = zip.archive.by_index(i) else { continue };
        if entry.size() == 0 {
            continue;
        }
        // Not sized by what the zip says it holds: a damaged zip can claim gigabytes.
        let mut data = Vec::new();
        // A state whose bytes don't come out whole is left out, not the rest with it.
        if entry.read_to_end(&mut data).is_err() {
            continue;
        }
        let name = full.rsplit(['/', if cfg!(windows) { '\\' } else { '/' }]).next().unwrap_or(full);
        let ext = paths::extension(name).to_owned();
        let stem = paths::stem(name);
        let owner = owners.get(&name.to_lowercase()).or_else(|| owners.get(&stem.to_lowercase()));
        presets.push(match owner {
            Some(&owner) => {
                let o = doc.obj(owner);
                let (plugin, preset) = (o.str(F_DEVICE_NAME), o.str(F_DEVICE_PRESET));
                let track = names.of(doc, owner);
                let shown =
                    if preset.is_empty() { format!("{track} - {plugin}") } else { format!("{track} - {preset}") };
                PresetBlob::new(&shown, plugin, ext, data)
            }
            None => PresetBlob::new(stem, "Unknown plugin", ext, data),
        });
    }
    presets
}

/// Audio in the folders Bitwig keeps beside a project (samples, recordings, audio), and how many bounces it has.
fn add_project_folder_audio(dir: &Path, info: &mut ProjectInfo) {
    for sub in ["samples", "recordings", "audio"] {
        info.samples.extend(files_under(&dir.join(sub)).into_iter().filter(|f| textscan::is_audio_path(f)));
    }
    let bounce = dir.join("bounce");
    if bounce.is_dir() {
        let count = files_under(&bounce).iter().filter(|f| textscan::is_audio_path(f)).count();
        info.details.insert("Bounces".into(), count.to_string());
    }
}

/// Every file in a folder and the folders inside it, a folder's own files before what's in its folders (as .NET lists
/// them). Folders linked in (a shortcut folder, a junction) are gone into too, each once, however they link back.
/// Nothing for a folder that isn't there or can't be read.
fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut todo = std::collections::VecDeque::from([dir.to_path_buf()]);
    while let Some(d) = todo.pop_front() {
        // Where it really is, so a link back to a folder already listed isn't listed again.
        if !seen.insert(std::fs::canonicalize(&d).unwrap_or_else(|_| d.clone())) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() || (t.is_symlink() && path.is_dir()) => todo.push_back(path),
                Ok(_) => out.push(path.to_string_lossy().into_owned()),
                Err(_) => {}
            }
        }
    }
    out
}

// ---- When the file can't be decoded: a text scan ----------------------------------------------------------------

/// The folders Bitwig keeps a project's own files in, next to the .bwproject.
const PROJECT_FOLDERS: [&str; 6] = ["samples", "recordings", "bounce", "audio", "multi-samples", "wavetables"];

fn text_scan(
    raw: &[u8],
    dir: &Path,
    info: &mut ProjectInfo,
    why: Option<&str>,
    collected: Option<&HashSet<String>>,
    known: &dyn Fn(&str) -> bool,
) {
    info.confidence = Confidence::Heuristic;
    if !super::is_bitwig(raw) {
        info.warnings.push("The file doesn't start with Bitwig's header, so results may be incomplete.".into());
    }
    let why = why.map(|w| format!(" ({w})")).unwrap_or_default();
    info.warnings.push(format!(
        "This project couldn't be fully decoded{why}, so plugins and samples were found by scanning text inside the \
         file. MIDI and preset extraction aren't available for it."
    ));

    each_blob(raw, |blob| {
        let mut expect_version = false;
        for found in scan_strings(blob) {
            let s = found.trim_matches(['\0', ' ']);
            // The version follows its key, either in the same run of text or as the next one.
            if let Some(v) = version_after_key(s) {
                info.version = v.to_owned();
            } else if expect_version && is_version(s) {
                info.version = s.to_owned();
            }
            expect_version = s.ends_with("application_version_name");
            if textscan::is_audio_path(s) {
                // As in a full read, an original that was collected into the project counts once, as its copy.
                if let Some(p) = scanned_path(s) {
                    let original = paths::is_absolute(p);
                    if !(original && collected.is_some_and(|c| c.contains(&paths::file_name(p).to_lowercase()))) {
                        info.samples.push(if original { p.to_owned() } else { paths::full_path(dir, p) });
                    }
                }
            } else if textscan::is_plugin_path(s) {
                let p = scanned_path(s).unwrap_or(s);
                let (name, format) = (plugins::name_from_path(p), plugins::format_from_path(p));
                info.plugins.push(plugin(&name, "", PluginKind::Unknown, format, p));
            } else if known(s) {
                info.plugins.push(plugin(s, "", plugins::kind_of(s), "", ""));
            } else if INSTRUMENTS.contains(&s) {
                info.plugins.push(plugin(s, "Bitwig", PluginKind::Instrument, "Native", ""));
            } else if EFFECTS.contains(&s) {
                info.plugins.push(plugin(s, "Bitwig", PluginKind::Effect, "Native", ""));
            } else if NOTE_EFFECTS.contains(&s) {
                info.plugins.push(plugin(s, "Bitwig", PluginKind::NoteEffect, "Native", ""));
            }
        }
    });
}

/// "1.2.3", or "5" (`application_version_name\W+(\d+(?:\.\d+)*)`): the version after the key, with something that
/// isn't a letter or digit between them.
fn version_after_key(s: &str) -> Option<&str> {
    const KEY: &str = "application_version_name";
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    for (at, _) in s.match_indices(KEY) {
        let rest = &s[at + KEY.len()..];
        let gap = rest.find(|c: char| is_word(c)).unwrap_or(rest.len());
        if gap == 0 || !rest[gap..].starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let v = &rest[gap..];
        // Digits, then each dot that has digits after it.
        let mut end = v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len());
        while v[end..].starts_with('.') && v[end + 1..].starts_with(|c: char| c.is_ascii_digit()) {
            end += 1 + v[end + 1..].find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len() - end - 1);
        }
        return Some(&v[..end]);
    }
    None
}

/// Digits with at least one dot between them, and nothing else: "6.1".
fn is_version(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// A path the text scan found, without what was read along with it: Bitwig writes a string's length just before it,
/// and a length that happens to be a readable letter ("D" for 68) joins the text ("DF:\Samples\..."). None for a bare
/// file name: those name the sounds inside a multisample or a Grid's sample list, not files on disk.
fn scanned_path(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let drive = b.windows(3).position(|w| matches!(w, [d, b':', b'\\' | b'/'] if d.is_ascii_alphabetic()));
    let s = match drive {
        Some(i) if i > 0 => &s[i..],
        _ => {
            let units = s.encode_utf16().count();
            let rest = s.char_indices().nth(1).map_or("", |(i, _)| &s[i..]);
            let first_is_length = units > 2 && s.encode_utf16().next() == Some(((units - 1) & 0xff) as u16);
            let looks_like_path = rest.starts_with('/')
                || rest.starts_with(r"\\")
                || PROJECT_FOLDERS.iter().any(|f| {
                    let r = rest.as_bytes();
                    r.get(f.len()) == Some(&b'/')
                        && r.get(..f.len()).is_some_and(|p| p.eq_ignore_ascii_case(f.as_bytes()))
                });
            if first_is_length && looks_like_path { rest } else { s }
        }
    };
    (s.contains('/') || s.contains('\\')).then_some(s)
}

/// The text in a blob. Bitwig writes a string with a character outside ASCII ("Beyoncé") as UTF-16 (big-endian), with
/// the top bit of its length set; those are read whole first and blanked out, so the plain scan doesn't find pieces of
/// them.
fn scan_strings(blob: &[u8]) -> Vec<String> {
    let mut bytes = blob.to_vec();
    let mut wide = Vec::new();
    let mut i = 0;
    while let Some(&[top, zero, high, low]) = bytes.get(i..).and_then(|b| b.first_chunk::<4>()) {
        let chars = (high as usize) << 8 | low as usize;
        let text = bytes.get(i + 4..i + 4 + 2 * chars);
        let Some(text) = text.filter(|_| top == 0x80 && zero == 0 && (4..=4096).contains(&chars)) else {
            i += 1;
            continue;
        };
        let units: Vec<u16> = text.as_chunks::<2>().0.iter().map(|&c| u16::from_be_bytes(c)).collect();
        let s: String = char::decode_utf16(units.iter().copied()).map(|c| c.unwrap_or('\u{FFFD}')).collect();
        let ascii = units.iter().filter(|&&u| u < 0x80).count();
        if s.chars().any(|c| c.is_control() || c == '\u{FFFD}')
            || (ascii * 2 < units.len() && !textscan::is_audio_path(&s) && !textscan::is_plugin_path(&s))
        {
            i += 1;
            continue;
        }
        wide.push(s);
        if let Some(read) = bytes.get_mut(i..i + 4 + 2 * chars) {
            read.fill(0);
        }
        i += 4 + 2 * chars;
    }
    let mut out = textscan::strings(&bytes);
    out.extend(wide);
    out
}

/// Hands over the raw file, then each zlib stream inside it (some Bitwig versions keep parts of a project
/// compressed), one at a time, so only one stream is unpacked in memory at once.
fn each_blob(raw: &[u8], mut scan: impl FnMut(&[u8])) {
    const MOST: u64 = 200_000_000;
    scan(raw);
    let mut attempts = 0;
    let mut i = 0;
    while let Some(rest) = raw.get(i..).filter(|r| r.len() > 2 && attempts < 200) {
        // A zlib header: 0x78, and the two bytes as a number divisible by 31.
        if let [0x78, second, ..] = *rest
            && (0x7800 | second as u32).is_multiple_of(31)
        {
            attempts += 1;
            let mut inflated = Vec::new();
            // Random bytes that happen to look like a zlib header fail in several ways; those are skipped.
            let read = flate2::read::ZlibDecoder::new(rest).take(MOST).read_to_end(&mut inflated);
            if read.is_ok() && inflated.len() >= 8 {
                scan(&inflated);
            }
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collected_file_had_its_own_name_and_each_shorter_one() {
        assert_eq!(collected_names("samples/Snare-2.wav"), ["Snare-2.wav", "Snare.wav"]);
        assert_eq!(collected_names("samples/Kick-01-2.wav"), ["Kick-01-2.wav", "Kick-01.wav", "Kick.wav"]);
        assert_eq!(collected_names("Pad-.wav"), ["Pad-.wav"]);
    }

    #[test]
    fn hardware_names_sort_about_as_dotnet_sorts_them() {
        let mut names = ["Box1", "analog", "Box_1", "Zebra", "box 2", "Analog Heat"];
        names.sort_by(|a, b| culture_order(a, b));
        assert_eq!(names, ["analog", "Analog Heat", "box 2", "Box_1", "Box1", "Zebra"]);
    }

    #[test]
    fn finds_the_version_after_its_key() {
        assert_eq!(version_after_key("application_version_name\0\0\u{5}5.2.4"), Some("5.2.4"));
        assert_eq!(version_after_key("application_version_name 6."), Some("6"));
        assert_eq!(version_after_key("application_version_namex 6"), None);
        assert!(is_version("6.1") && !is_version("6") && !is_version("6.") && !is_version("a.1"));
    }

    #[test]
    fn a_scanned_path_loses_the_length_read_with_it() {
        assert_eq!(scanned_path(r">F:\Samples\a.wav"), Some(r"F:\Samples\a.wav"));
        let collected = "recordings/Untitled Project [0m27s-1m22s].wav";
        assert_eq!(scanned_path(&format!("-{collected}")), Some(collected));
        assert_eq!(scanned_path("-recordings/a.wav"), Some("-recordings/a.wav"));
        assert_eq!(scanned_path("BASSDRUM.wav"), None);
        assert_eq!(scanned_path(r"F:\a.wav"), Some(r"F:\a.wav"));
    }
}
