//! Writes a [`Plan`] as a native Bitwig project (`.bwproject`): the C# `BitwigProject` writer.
//!
//! Bitwig's format isn't published, so the project is built from a template Bitwig saved (Solomon's, never in the repo:
//! rule 4) with a Sampler track, a VST3 instrument track with a Note Transpose before the plugin and a clip of notes, a
//! VST2 instrument track and, for automation, a track with its volume automated; for audio clips, an audio track with
//! an audio clip on it. Those tracks, devices, clips and lanes are taken out of the template and copied for each track
//! in the plan, then filled in; the template's own tracks, effect tracks and sends are left out. Its master stays, with
//! anything on it.
//!
//! The template is only read; the project is new bytes, which the caller saves as a new file (rule 3).

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};

use mo_formats::bitwig::{self, Blob, Copier, Document, ObjId, Object, Slot, Text, Value};
use mo_formats::fl::song::FlPlugin;
use mo_formats::paths;
// Moved to mo-formats with the presets; kept here under its old name.
pub use mo_formats::presets::fxp_to_bank;

use crate::plan::{AudioClip, Device, Lane, LanePoint, Plan, Role, SampleFile, Target, Track, TrackKind, TrackRef};

const SAMPLER_ID: &str = "468bc14b-b2e7-45a1-9666-e83117fe404e";
const NOTE_TRANSPOSE_ID: &str = "0815cd9e-3a31-4429-a268-dabd952a3b68";
/// Plugin device classes.
const VST3: u32 = 0x728;
const VST2: u32 = 0x1a5;
/// Device kinds (field 0x9d).
const AUDIO_EFFECT: u8 = 0;
const INSTRUMENT: u8 = 2;
/// An arrangement audio clip (a clip of notes is 0x47).
const AUDIO_CLIP_CLASS: u32 = 0xee;
/// An audio track in the project's summary (instrument tracks are 0x288).
const AUDIO_ENTRY: u32 = 0x287;
const STATES: &str = "plugin-states/";

/// A converted project, and what couldn't come across.
#[derive(Debug)]
pub struct Converted {
    pub bytes: Vec<u8>,
    pub notes: Vec<String>,
}

/// Why a project can't be the template (for example "The Bitwig template needs a VST2 instrument track"), or `None`
/// when it can.
pub fn template_problem(template: &[u8]) -> Option<String> {
    Template::new(template).err()
}

/// Writes the plan into a copy of the template. The plan's notes come first, then what the template couldn't carry.
pub fn write(plan: &Plan, template: &[u8]) -> Result<Converted, String> {
    let mut t = Template::new(template)?;
    let mut w = Writing { notes: plan.notes.clone(), states: Vec::new() };

    // Each plan track as a Bitwig track, and each plan device as its object, by place. An audio track the template
    // can't make is left out and said.
    let mut tracks: Vec<Option<(ObjId, Vec<Option<ObjId>>)>> = Vec::with_capacity(plan.tracks.len());
    for track in &plan.tracks {
        if track.kind == TrackKind::Audio && t.audio.is_none() {
            w.notes.push(format!(
                "{}: its audio clips are left out (the Bitwig template has no audio track with an audio clip on it that \
                 can be copied; it can't be in a group)",
                track.name
            ));
            tracks.push(None);
            continue;
        }
        tracks.push(Some(w.track(&mut t, track, plan.ppq)?));
    }
    let master = t.master;
    let mut master_devices = Vec::with_capacity(plan.master_effects.len());
    let mut chain = t.chain(master);
    for device in &plan.master_effects {
        let made = w.device(&mut t, device)?;
        if let Some(d) = made {
            chain.push(d);
        }
        master_devices.push(made);
    }
    if !plan.master_effects.is_empty() {
        t.set_chain(master, &chain);
    }

    for lane in &plan.lanes {
        let (track, devices): (ObjId, &[Option<ObjId>]) = match lane.track {
            TrackRef::Master => (master, &master_devices),
            // The transport has no devices.
            TrackRef::Song => match t.transport() {
                Some(transport) => (transport, &[]),
                None => {
                    for clip in &lane.clips {
                        w.notes.push(format!("{clip}: automation left out (the Bitwig template has no tempo setting)"));
                    }
                    continue;
                }
            },
            TrackRef::Track(i) => match tracks.get(i) {
                Some(Some((track, devices))) => (*track, devices),
                // Its track is left out, and said.
                _ => continue,
            },
        };
        // Bitwig's tempo runs from 20 BPM; below it the lane stays at 20, from where FL's line crosses it.
        let slowest = (lane.target == Target::Tempo).then(|| at_least(&lane.points, MIN_BPM));
        let clamped = slowest.as_ref().is_some_and(|(_, clamped)| *clamped);
        let raised;
        let lane = match slowest {
            Some((points, true)) => {
                raised = Lane { points, ..lane.clone() };
                &raised
            }
            _ => lane,
        };
        match t.add_lane(track, devices, lane) {
            Some(why) => {
                // FL's reader leaves an effect that's switched on and off switched on.
                let on = if matches!(lane.target, Target::OnOff { .. }) { ", so the effect stays on" } else { "" };
                for clip in &lane.clips {
                    w.notes.push(format!("{clip}: automation left out ({why}){on}"));
                }
            }
            None if clamped => {
                for clip in &lane.clips {
                    w.notes.push(format!(
                        "{clip}: Bitwig's tempo goes down to {MIN_BPM} BPM, so the slower part plays at {MIN_BPM} BPM"
                    ));
                }
            }
            None => {}
        }
    }

    let made: Vec<ObjId> = tracks.iter().flatten().map(|(t, _)| *t).collect();
    let bytes = t.finish(&made, plan.tempo, &w.states)?;
    let mut seen = HashSet::new();
    let notes = w.notes.into_iter().filter(|n| seen.insert(n.clone())).collect();
    Ok(Converted { bytes, notes })
}

/// The slowest tempo Bitwig takes, in BPM.
const MIN_BPM: f64 = 20.0;

/// A lane's points kept at `min` or above, with a point put in where the straight line between two points crosses
/// `min`; and whether any point was raised.
fn at_least(points: &[LanePoint], min: f64) -> (Vec<LanePoint>, bool) {
    let mut out: Vec<LanePoint> = Vec::with_capacity(points.len());
    let mut last: Option<LanePoint> = None;
    for &q in points {
        if let Some(p) = last
            && q.beat > p.beat + SAME_BEAT
            && (p.value < min) != (q.value < min)
        {
            let part = (min - p.value) / (q.value - p.value);
            out.push(LanePoint { beat: p.beat + (q.beat - p.beat) * part, value: min, ..q });
        }
        out.push(LanePoint { value: q.value.max(min), ..q });
        last = Some(q);
    }
    (out, points.iter().any(|p| p.value < min))
}

struct Writing {
    notes: Vec<String>,
    /// The plugin settings to put in the project's zip, by their file name there.
    states: Vec<(String, Vec<u8>)>,
}

impl Writing {
    fn track(&mut self, t: &mut Template, plan: &Track, ppq: u16) -> Result<(ObjId, Vec<Option<ObjId>>), String> {
        let vst2 = plan
            .devices
            .iter()
            .any(|d| matches!(d, Device::Plugin { plugin, role: Role::Instrument, .. } if plugin.format == "VST2"));
        let pattern = match plan.kind {
            TrackKind::Sampler => t.sampler_track,
            TrackKind::Audio => t.audio.as_ref().map(|a| a.track).ok_or("The Bitwig template has no audio track")?,
            TrackKind::Instrument if vst2 => t.vst2_track,
            TrackKind::Instrument => t.vst3_track,
        };
        let track = t.copy(pattern);
        // The copied track's own Sampler is filled in, rather than another copy made.
        let own_sampler = (plan.kind == TrackKind::Sampler).then(|| t.chain(track).first().copied()).flatten();
        let mut chain = Vec::new();
        let mut devices = Vec::with_capacity(plan.devices.len());
        for device in &plan.devices {
            let made = match device {
                Device::Sampler { sample, info, root_key, tune } => {
                    let s = own_sampler.ok_or("The template's Sampler track has no Sampler")?;
                    t.sampler(s, sample, info.as_ref(), *root_key, *tune)?;
                    Some(s)
                }
                _ => self.device(t, device)?,
            };
            if let Some(d) = made {
                chain.push(d);
            }
            devices.push(made);
        }
        t.set_chain(track, &chain);
        t.doc.obj_mut(track).set(0x15b, text(&plan.name));
        t.mixer(track, plan);
        for clip in &plan.clips {
            t.add_clip(track, clip, ppq);
        }
        if let Some((path, info)) = &plan.sample {
            for clip in &plan.audio_clips {
                t.add_audio_clip(track, &plan.source, path, info.as_ref(), clip)?;
            }
        }
        if plan.kind == TrackKind::Audio {
            t.audio_tracks.insert(track);
        }
        Ok((track, devices))
    }

    /// A transpose or plugin as a Bitwig device. A plugin is a copy of the template's VST3 or VST2 device with this
    /// plugin's identity and settings; an effect switched off in FL is switched off here too.
    fn device(&mut self, t: &mut Template, device: &Device) -> Result<Option<ObjId>, String> {
        match device {
            Device::Sampler { .. } => Ok(None),
            Device::Transpose { octaves, semitones } => {
                let d = t.copy(t.note_transpose);
                let def = t.def(d).ok_or("The template's Note Transpose has no settings")?;
                if let Some(p) = t.param(def, "OCTAVES") {
                    put(t.doc.obj_mut(p), 0x273, Value::U8((octaves + 3).clamp(0, 6) as u8));
                }
                if let Some(p) = t.param(def, "COARSE") {
                    // A signed byte: -4 is 252.
                    put(t.doc.obj_mut(p), 0x330, Value::U8(*semitones as i8 as u8));
                }
                Ok(Some(d))
            }
            Device::Plugin { plugin, role, enabled, insert } => {
                let kind = if *role == Role::Effect { AUDIO_EFFECT } else { INSTRUMENT };
                let d = self.plugin(t, plugin, kind)?;
                if !enabled {
                    let def = t.def(d).ok_or("a plugin device has no settings")?;
                    if t.doc.obj(def).has(0xa3) {
                        put(t.doc.obj_mut(def), 0xa3, Value::Bool(0));
                    } else {
                        self.notes.push(format!(
                            "{} on {insert}: switched off in FL Studio, but on here (the Bitwig template's plugin has \
                             no on/off to copy)",
                            plugin.name
                        ));
                    }
                }
                Ok(Some(d))
            }
        }
    }

    fn plugin(&mut self, t: &mut Template, p: &FlPlugin, kind: u8) -> Result<ObjId, String> {
        let vst3 = p.format == "VST3";
        let device = t.copy(if vst3 { t.vst3_device } else { t.vst2_device });
        let def = t.def(device).ok_or("a plugin device has no settings")?;
        let same_plugin = t.doc.obj(def).get(0xce4).is_some_and(|v| value_text(v) == p.device_id);
        {
            let o = t.doc.obj_mut(def);
            o.set(0x9a, text(&p.name));
            o.set(0x9b, text(&p.vendor));
            o.set(0x12de, text(""));
            put(o, 0x9d, Value::U8(kind));
            if kind == AUDIO_EFFECT {
                o.set(0x9c, text("Audio FX"));
                if o.has(0x1809) {
                    // As Bitwig saves effect plugins.
                    put(o, 0x1809, Value::Bool(0));
                }
            }
            if vst3 {
                put(o, 0xce4, text(&p.device_id));
            } else {
                // A VST2's id is a number; with none saved, 0 rather than the template plugin's, so Bitwig never loads
                // that plugin under this one's name.
                put(o, 0xce4, int(p.device_id.parse::<i64>().unwrap_or(0)));
            }
            if !same_plugin && o.has(0xc5e) {
                // Another plugin than the template's: its version and outputs aren't known, Bitwig reads them on load.
                put(o, 0xc5e, if vst3 { text("") } else { Value::U8(0) });
            }
            // The template's automated settings; this plugin's are added with their lanes.
            o.set(0x1318, Value::List(Vec::new()));
        }
        if !same_plugin {
            if let Some(outputs) = t.doc.field_obj(def, 0x137c) {
                t.doc.obj_mut(outputs).set(0x8e0, Value::List(Vec::new()));
            }
            if let Some(program) = t.doc.field_obj(def, 0x1b75) {
                t.doc.obj_mut(program).set(0x1b72, text(""));
            }
        }
        if let Some(location) = t.doc.field_obj(def, 0xc5a) {
            let list = t.doc.field_list(location, 0xc5b);
            match list.first() {
                Some(&file) if !p.path.is_empty() => {
                    t.doc.obj_mut(file).set(0x3b, text(&p.path));
                    t.doc.obj_mut(location).set(0xc5b, Value::List(vec![Slot::Obj(file)]));
                }
                _ => t.doc.obj_mut(location).set(0xc5b, Value::List(Vec::new())),
            }
        }
        let mut state = String::new();
        if let Some(s) = &p.state {
            let bank = (!vst3 && s.extension == ".fxp").then(|| fxp_to_bank(&s.data, s.program));
            // Saved as a bank only when it became one: a program Bitwig reads as a bank wouldn't load.
            let (data, extension) = match bank {
                Some(bank) if bank != s.data => (bank, ".fxb".to_owned()),
                _ => (s.data.clone(), s.extension.to_string()),
            };
            state = format!("{}{extension}", uuid_text(&bitwig::encode::fresh_id()));
            self.states.push((state.clone(), data));
        }
        t.doc.obj_mut(def).set(0xbe5, text(&state));
        Ok(device)
    }
}

/// Fills in a new lane's target: what it moves.
type SetUp = Box<dyn Fn(&mut Document, ObjId)>;

/// The setting a device's On/Off lane moves, as Bitwig names it.
const ON_OFF_SETTING: &str = "On";
/// Points closer than this are on the same beat (as in the plan).
const SAME_BEAT: f64 = 1e-4;

/// The template, with its tracks taken out and kept as patterns to copy.
struct Template {
    doc: Document,
    group: ObjId,
    master: ObjId,
    sampler_track: ObjId,
    vst3_track: ObjId,
    vst2_track: ObjId,
    vst3_device: ObjId,
    vst2_device: ObjId,
    note_transpose: ObjId,
    clip: ObjId,
    key_lane: ObjId,
    note: ObjId,
    mixer_lane: Option<ObjId>,
    plugin_lane: Option<ObjId>,
    point: Option<ObjId>,
    setting: Option<ObjId>,
    audio: Option<AudioPattern>,
    audio_tracks: HashSet<ObjId>,
    copier: Copier,
}

/// A track to copy for audio clips, and its audio clip, when the template has them.
struct AudioPattern {
    track: ObjId,
    clip: ObjId,
}

impl Template {
    fn new(raw: &[u8]) -> Result<Template, String> {
        let mut doc = bitwig::decode(raw)
            .map_err(|_| "The Bitwig template couldn't be read (it isn't a Bitwig file)".to_owned())?;
        let root = match (doc.root, &doc.body_error) {
            (Some(root), None) => root,
            (_, e) => {
                return Err(format!(
                    "The Bitwig template couldn't be read ({})",
                    e.as_deref().unwrap_or("no project in it")
                ));
            }
        };
        let group = doc.field_obj(root, 0x4dd).ok_or("The Bitwig template has no tracks")?;
        let master = doc.field_obj(group, 0x4e0).ok_or("The Bitwig template has no master track")?;
        let mut originals = doc.field_list(group, 0x4de);
        originals.extend(doc.field_list(group, 0x4df));

        let chain = |doc: &Document, track: ObjId| chain_of(doc, track);
        let def = |doc: &Document, d: ObjId| doc.field_obj(d, 0x197);
        let bitwig_id = |doc: &Document, d: ObjId| match def(doc, d).map(|x| doc.obj(x)) {
            Some(o) if o.class == 0x40 => match o.get(0x99) {
                Some(Value::Uuid(u)) => Some(*u),
                _ => None,
            },
            _ => None,
        };
        let vst = |doc: &Document, d: ObjId, class: u32| {
            def(doc, d).map(|x| doc.obj(x)).is_some_and(|o| o.class == class && o.num(0x9d) == Some(INSTRUMENT as f64))
        };
        let find = |doc: &Document, test: &dyn Fn(&Document, ObjId) -> bool| {
            originals.iter().copied().find(|&tr| chain(doc, tr).iter().any(|&d| test(doc, d)))
        };
        let sampler = uuid_bytes(SAMPLER_ID);
        let transpose = uuid_bytes(NOTE_TRANSPOSE_ID);
        let mut missing = Vec::new();
        let sampler_track = find(&doc, &|doc, d| bitwig_id(doc, d) == Some(sampler));
        let vst3_track = find(&doc, &|doc, d| vst(doc, d, VST3));
        let vst2_track = find(&doc, &|doc, d| vst(doc, d, VST2));
        let note_transpose =
            originals.iter().flat_map(|&tr| chain(&doc, tr)).find(|&d| bitwig_id(&doc, d) == Some(transpose));
        // A clip of notes, the VST3 track's first; an audio clip's content is laid out the same, so it's told apart by
        // its class.
        let mut ordered = originals.clone();
        ordered.sort_by_key(|&tr| Some(tr) != vst3_track);
        let clip = ordered.iter().flat_map(|&tr| clips_of(&doc, tr)).find(|&c| {
            doc.obj(c).class != AUDIO_CLIP_CLASS
                && key_lanes(&doc, c).iter().any(|&l| !doc.field_list(l, 0x21f).is_empty())
        });
        for (found, what) in [
            (sampler_track.is_some(), "a Sampler track"),
            (vst3_track.is_some(), "a VST3 instrument track"),
            (vst2_track.is_some(), "a VST2 instrument track"),
            (note_transpose.is_some(), "a Note Transpose"),
            (clip.is_some(), "a clip with notes"),
        ] {
            if !found {
                missing.push(what);
            }
        }
        let (Some(sampler_track), Some(vst3_track), Some(vst2_track), Some(note_transpose), Some(clip)) =
            (sampler_track, vst3_track, vst2_track, note_transpose, clip)
        else {
            return Err(format!("The Bitwig template needs {}", missing.join(", ")));
        };
        let vst3_device = chain(&doc, vst3_track).into_iter().find(|&d| vst(&doc, d, VST3)).ok_or("no VST3")?;
        let vst2_device = chain(&doc, vst2_track).into_iter().find(|&d| vst(&doc, d, VST2)).ok_or("no VST2")?;
        if sampler_track == vst3_track || sampler_track == vst2_track || vst3_track == vst2_track {
            return Err(
                "The Bitwig template needs its Sampler, VST3 and VST2 instruments on tracks of their own".into()
            );
        }
        for tr in [sampler_track, vst3_track, vst2_track] {
            if doc.field_obj(tr, 0x2bf8).and_then(|a| doc.field_obj(a, 0x2d8e)).is_none() {
                return Err(format!(
                    "The Bitwig template's track \"{}\" has no arrangement to put clips in",
                    doc.obj(tr).str(0x15b)
                ));
            }
        }
        let first_sampler = chain(&doc, sampler_track).into_iter().find(|&d| bitwig_id(&doc, d) == Some(sampler));
        let sample_zone = first_sampler
            .and_then(|d| def(&doc, d))
            .and_then(|d| doc.field_obj(d, 0xa4))
            .and_then(|c| doc.field_list(c, 0x20c).into_iter().find(|&p| doc.obj(p).str(0x2b9) == "SAMPLE"))
            .and_then(|p| doc.field_obj(p, 0x74c))
            .and_then(|z| doc.field_obj(z, 0x4112))
            .and_then(|s| doc.field_obj(s, 0x748));
        if !sample_zone.is_some_and(|s| doc.obj(s).has(0x129e)) {
            return Err("The Bitwig template's Sampler has no sample in it".into());
        }
        let key_lane = key_lanes(&doc, clip)
            .into_iter()
            .find(|&l| !doc.field_list(l, 0x21f).is_empty())
            .ok_or("The Bitwig template's clip has no notes")?;
        let note = *doc.field_list(key_lane, 0x21f).first().ok_or("The Bitwig template's clip has no notes")?;

        // Automation lanes on a mixer setting and on a plugin's setting, and a point, if the template has them; and how
        // a plugin lists a setting it has automated.
        let lanes: Vec<ObjId> = originals
            .iter()
            .filter_map(|&tr| doc.field_obj(tr, 0x2dc9))
            .flat_map(|l| doc.field_list(l, 0x4ba))
            .filter(|&l| doc.field_obj(l, 0x2d8c).and_then(|a| doc.field_obj(a, 0x35ac)).is_some())
            .collect();
        fn target(doc: &Document, l: ObjId) -> Option<&Object> {
            doc.field_obj(l, 0x2a3).map(|t| doc.obj(t))
        }
        let mut mixer_lane = lanes.iter().copied().find(|&l| target(&doc, l).is_some_and(|t| t.has(0x41)));
        let mut plugin_lane = lanes.iter().copied().find(|&l| {
            // Not a device's On/Off lane, whose points are switches rather than numbers.
            target(&doc, l).is_some_and(|t| t.has(0xec) && t.has(0xed) && t.str(0xed) != ON_OFF_SETTING)
        });
        let point = lanes
            .iter()
            .filter_map(|&l| doc.field_obj(l, 0x2d8c).and_then(|a| doc.field_obj(a, 0x35ac)))
            .flat_map(|points| doc.field_list(points, 0x21f))
            .find(|&o| doc.obj(o).has(0x2af) && doc.obj(o).has(0x28f));
        if point.is_none() {
            (mixer_lane, plugin_lane) = (None, None);
        }
        let setting = [vst3_device, vst2_device]
            .iter()
            .filter_map(|&d| def(&doc, d))
            .flat_map(|d| doc.field_list(d, 0x1318))
            .find(|&o| doc.obj(o).has(0x1319));
        // An audio track and an audio clip with a sample, on a track of its own, if the template has them.
        let audio_track = originals
            .iter()
            .copied()
            .filter(|tr| ![sampler_track, vst3_track, vst2_track].contains(tr))
            .find(|&tr| clips_of(&doc, tr).iter().any(|&c| is_audio_clip(&doc, c)));
        let mut audio = audio_track.and_then(|track| {
            let clip = clips_of(&doc, track).into_iter().find(|&c| is_audio_clip(&doc, c))?;
            Some(AudioPattern { track, clip })
        });

        // Take the template's tracks out of the project, with what points into them: the per-user view state (selected
        // track, editors), clip selections, sends and effect tracks. They stay as patterns with only their instrument,
        // no clips and no automation lanes.
        if doc.obj(root).has(0x4d) {
            doc.obj_mut(root).set(0x4d, Value::Map(Vec::new()));
        }
        if let Some(selection) = doc.field_obj(group, 0x2bfb).and_then(|s| doc.field_obj(s, 0x2d8b)) {
            doc.obj_mut(selection).set(0x21f, Value::List(Vec::new()));
        }
        doc.obj_mut(group).set(0x4de, Value::List(Vec::new()));
        doc.obj_mut(group).set(0x4df, Value::List(Vec::new()));
        for &tr in &originals {
            if doc.obj(tr).has(0x168) {
                doc.obj_mut(tr).set(0x168, Value::Map(Vec::new()));
            }
            for (holder, fields) in [
                (vec![0x165], &[0xa5][..]),
                (vec![0x2dc9], &[0x4ba, 0x3f83]),
                (vec![0x2aed], &[0x10e6]),
                (vec![0x2bf8, 0x2d8e], &[0x21f]),
            ] {
                let mut at = Some(tr);
                for f in holder {
                    at = at.and_then(|o| doc.field_obj(o, f));
                }
                if let Some(o) = at {
                    for &f in fields {
                        doc.obj_mut(o).set(f, Value::List(Vec::new()));
                    }
                }
            }
        }
        set_chain_of(&mut doc, sampler_track, &first_sampler.into_iter().collect::<Vec<_>>());
        // The plugin devices are kept apart and copied as each track needs them, so the tracks hold none.
        set_chain_of(&mut doc, vst3_track, &[]);
        set_chain_of(&mut doc, vst2_track, &[]);
        if let Some(a) = &audio {
            set_chain_of(&mut doc, a.track, &[]);
        }
        // Sends to effect tracks inside devices (a multi-out plugin's output chains) go with the effect tracks.
        let kept: Vec<ObjId> =
            [Some(sampler_track), Some(vst3_track), Some(vst2_track), audio.as_ref().map(|a| a.track)]
                .into_iter()
                .chain([Some(vst3_device), Some(vst2_device)])
                .flatten()
                .flat_map(|tr| reachable(&doc, tr))
                .collect();
        for o in kept {
            let sends: Vec<(usize, Vec<Slot>)> = doc
                .obj(o)
                .fields
                .iter()
                .enumerate()
                .filter_map(|(i, f)| match &f.value {
                    Value::List(l) => {
                        let kept: Vec<Slot> = l
                            .iter()
                            .copied()
                            .filter(|&s| {
                                !doc.resolve(s).is_some_and(|x| doc.obj(x).str(0x2b9).starts_with("TRACK_SEND("))
                            })
                            .collect();
                        (kept.len() != l.len()).then_some((i, kept))
                    }
                    _ => None,
                })
                .collect();
            for (i, kept) in sends {
                if let Some(f) = doc.obj_mut(o).fields.get_mut(i) {
                    f.value = Value::List(kept);
                }
            }
        }
        doc.obj_mut(key_lane).set(0x21f, Value::List(Vec::new()));
        if let Some(content) = note_content(&doc, clip) {
            doc.obj_mut(content).set(0x18cb, Value::List(Vec::new()));
        }
        if let Some(a) = &audio {
            // Only the audio that's filled in: anything else in the template's clip would play its own sample.
            let lane = key_lanes(&doc, a.clip).first().copied();
            let event = audio_event(&doc, a.clip);
            if let (Some(lane), Some(event), Some(content)) = (lane, event, note_content(&doc, a.clip)) {
                doc.obj_mut(lane).set(0x21f, Value::List(vec![Slot::Obj(event)]));
                doc.obj_mut(content).set(0x18cb, Value::List(vec![Slot::Obj(lane)]));
            }
        }
        for (lane, moves) in [(mixer_lane, 0x41), (plugin_lane, 0xec)] {
            let Some(lane) = lane else { continue };
            // No clips, points or per-user state, and nothing to move until it's given something.
            if let Some(arrangement) = doc.field_obj(lane, 0x2d8c) {
                doc.obj_mut(arrangement).set(0x21f, Value::List(Vec::new()));
                if let Some(points) = doc.field_obj(arrangement, 0x35ac) {
                    doc.obj_mut(points).set(0x21f, Value::List(Vec::new()));
                }
            }
            if let Some(contents) = doc.field_obj(lane, 0x2aec) {
                doc.obj_mut(contents).set(0x10e6, Value::List(Vec::new()));
            }
            if doc.obj(lane).has(0x4bc) {
                doc.obj_mut(lane).set(0x4bc, Value::Map(Vec::new()));
            }
            if let Some(target) = doc.field_obj(lane, 0x2a3) {
                put(doc.obj_mut(target), moves, Value::Null);
            }
        }

        let copier = Copier::new(&doc, None);
        let patterns =
            [sampler_track, vst3_track, vst2_track, vst3_device, vst2_device, note_transpose, clip, key_lane, note]
                .into_iter()
                .map(Some)
                .chain([mixer_lane, plugin_lane, point, setting]);
        if patterns.flatten().any(|o| copier.shares(o)) {
            return Err("The Bitwig template refers to its tracks from somewhere this app doesn't know to clear".into());
        }
        // Audio clips are filled in where they're copied, so nothing in one may be shared with the rest of the project.
        if audio
            .as_ref()
            .is_some_and(|a| copier.shares(a.track) || reachable(&doc, a.clip).iter().any(|&o| copier.shares(o)))
        {
            audio = None;
        }
        Ok(Template {
            doc,
            group,
            master,
            sampler_track,
            vst3_track,
            vst2_track,
            vst3_device,
            vst2_device,
            note_transpose,
            clip,
            key_lane,
            note,
            mixer_lane,
            plugin_lane,
            point,
            setting,
            audio,
            audio_tracks: HashSet::new(),
            copier,
        })
    }

    fn copy(&mut self, pattern: ObjId) -> ObjId {
        self.copier.copy(&mut self.doc, pattern)
    }

    fn def(&self, device: ObjId) -> Option<ObjId> {
        self.doc.field_obj(device, 0x197)
    }

    /// A device's named setting, from its CONTENTS list.
    fn param(&self, def: ObjId, name: &str) -> Option<ObjId> {
        let contents = self.doc.field_obj(def, 0xa4)?;
        self.doc.field_list(contents, 0x20c).into_iter().find(|&p| self.doc.obj(p).str(0x2b9) == name)
    }

    fn chain(&self, track: ObjId) -> Vec<ObjId> {
        chain_of(&self.doc, track)
    }

    fn set_chain(&mut self, track: ObjId, devices: &[ObjId]) {
        set_chain_of(&mut self.doc, track, devices);
    }

    /// Fills in a Sampler: its sample, root key and tune (FL's pitch knob, in semitones).
    fn sampler(
        &mut self,
        sampler: ObjId,
        path: &str,
        info: Option<&SampleFile>,
        root: u8,
        tune: f64,
    ) -> Result<(), String> {
        let def = self.def(sampler).ok_or("The template's Sampler has no settings")?;
        let stem = paths::stem(paths::file_name(path)).to_owned();
        self.doc.obj_mut(def).set(0x12de, text(&stem));
        self.doc.obj_mut(sampler).set(0x199, text(&stem));
        if let Some(p) = self.param(def, "TRANSPOSE") {
            self.doc.obj_mut(p).set(0x136, Value::F64(tune));
        }
        let zone = self.param(def, "SAMPLE").and_then(|p| self.doc.field_obj(p, 0x74c));
        let sample = zone.and_then(|z| self.doc.field_obj(z, 0x4112)).and_then(|s| self.doc.field_obj(s, 0x748));
        let (Some(zone), Some(sample)) = (zone, sample) else {
            return Err("The template's Sampler has no sample in it".into());
        };
        if !self.sample_is(sample, path, info) {
            return Err("The template's Sampler has no sample in it".into());
        }
        put(self.doc.obj_mut(zone), 0x75c, Value::U8(root.min(127)));
        if let Some(i) = info.filter(|i| i.seconds > 0.0) {
            self.doc.obj_mut(zone).set(0x75f, Value::F64(i.seconds));
            self.doc.obj_mut(zone).set(0x761, Value::F64(i.seconds));
        }
        Ok(())
    }

    /// Points a sample Bitwig saved (a Sampler's, or an audio clip's) at another file: its name, path, and what Bitwig
    /// recognises it by. When the file couldn't be read, or how long it is couldn't be told, its MD5 is zeros, so
    /// Bitwig asks where it is and reads it itself rather than trusting a made-up length. False when `sample` isn't
    /// a sample.
    fn sample_is(&mut self, sample: ObjId, path: &str, info: Option<&SampleFile>) -> bool {
        let Some(file) = self.doc.field_obj(sample, 0x129e) else { return false };
        let name = paths::file_name(path).to_owned();
        self.doc.obj_mut(sample).set(0x129f, text(&name));
        self.doc.obj_mut(file).set(0x512, text(&name));
        if let Some(location) = self.doc.field_obj(file, 0xd3b) {
            self.doc.obj_mut(location).set(0x3b, text(path));
        }
        let hash = self.doc.field_obj(file, 0x513);
        let Some(i) = info.filter(|i| i.seconds > 0.0) else {
            if let Some(h) = hash {
                put(self.doc.obj_mut(h), 0x16, Value::Blob(Box::new(Blob::new(&[0; 16]))));
            }
            return true;
        };
        if let Some(h) = hash {
            put(self.doc.obj_mut(h), 0x16, Value::Blob(Box::new(Blob::new(&i.md5))));
        }
        put(self.doc.obj_mut(file), 0x17, int(i.bytes.min(i64::MAX as u64) as i64));
        let s = self.doc.obj_mut(sample);
        put(s, 0x94, int(i.rate.into()));
        s.set(0x95, Value::F64(i.seconds));
        put(s, 0x96, Value::U8(i.channels.min(255) as u8));
        true
    }

    /// The track's volume (Bitwig's fader holds the cube root of the gain) and pan, and its mute.
    fn mixer(&mut self, track: ObjId, plan: &Track) {
        let Some(mixer) = self.doc.field_obj(track, 0x165) else { return };
        if let Some(v) = self.doc.field_obj(mixer, 0x1a4) {
            self.doc.obj_mut(v).set(0x2c8, Value::F64(plan.gain.cbrt()));
        }
        if let Some(p) = self.doc.field_obj(mixer, 0x1a5) {
            self.doc.obj_mut(p).set(0x2c8, Value::F64(plan.pan));
        }
        // Its mute button is on when MUTE's value is 1.
        if let Some(m) = self.doc.field_obj(mixer, 0x1a6)
            && (plan.muted || self.doc.obj(m).has(0xd2))
        {
            put(self.doc.obj_mut(m), 0xd2, Value::Bool(plan.muted.into()));
        }
    }

    /// One clip as an arrangement clip, muted when it is in FL. Notes sit in one lane per key, highest first, as Bitwig
    /// saves them, and are muted as in FL's piano roll.
    fn add_clip(&mut self, track: ObjId, fl: &mo_formats::fl::song::FlClip, ppq: u16) {
        let beats = |ticks: i64| ticks as f64 / ppq.max(1) as f64;
        let length = beats(fl.length);
        let clip = self.copy(self.clip);
        {
            let c = self.doc.obj_mut(clip);
            c.set(0x2af, Value::F64(beats(fl.start)));
            c.set(0x26, Value::F64(length));
            if fl.muted || c.has(0x10f8) {
                put(c, 0x10f8, Value::Bool(fl.muted.into()));
            }
        }
        // The loop and play range are timed from the clip's start.
        if let Some(range) = self.doc.field_obj(clip, 0x2adb) {
            self.set_range(range, 0.0, length);
        }
        // Highest key first, each key's notes in time order (a stable sort, so notes on one tick keep theirs).
        let mut sorted: Vec<_> = fl.notes.iter().collect();
        sorted.sort_by_key(|n| (std::cmp::Reverse(n.key.min(127)), n.start));
        let mut lanes = Vec::new();
        for on_key in sorted.chunk_by(|a, b| a.key.min(127) == b.key.min(127)) {
            let Some(key) = on_key.first().map(|n| n.key.min(127)) else { continue };
            let lane = self.copy(self.key_lane);
            put(self.doc.obj_mut(lane), 0xee, Value::U8(key));
            let mut notes = Vec::with_capacity(on_key.len());
            for n in on_key {
                let note = self.copy(self.note);
                let velocity = n.velocity as f64 / mo_formats::velocity::MAX as f64;
                let o = self.doc.obj_mut(note);
                o.set(0x2af, Value::F64(beats(n.start)));
                o.set(0x26, Value::F64(beats(n.length)));
                if n.muted || o.has(0x10f8) {
                    put(o, 0x10f8, Value::Bool(n.muted.into()));
                }
                o.set(0xef, Value::F64(velocity));
                o.set(0xf0, Value::F64(velocity));
                notes.push(Slot::Obj(note));
            }
            self.doc.obj_mut(lane).set(0x21f, Value::List(notes));
            lanes.push(Slot::Obj(lane));
        }
        if let Some(content) = note_content(&self.doc, clip) {
            self.doc.obj_mut(content).set(0x18cb, Value::List(lanes));
        }
        let content = self.doc.field_obj(clip, 0x2ae9).and_then(|c| self.doc.field_obj(c, 0x10e9));
        self.place(track, clip, content);
    }

    /// Puts a clip in the track's arrangement, and its content in the track's list of them.
    fn place(&mut self, track: ObjId, clip: ObjId, content: Option<ObjId>) {
        if let Some(arrangement) = self.doc.field_obj(track, 0x2bf8).and_then(|a| self.doc.field_obj(a, 0x2d8e)) {
            push(self.doc.obj_mut(arrangement), 0x21f, Slot::Obj(clip));
        }
        if let (Some(contents), Some(content)) = (self.doc.field_obj(track, 0x2aed), content) {
            push(self.doc.obj_mut(contents), 0x10e6, Slot::Obj(content));
        }
    }

    fn set_range(&mut self, range: ObjId, from: f64, length: f64) {
        if let Some(r) = self.doc.field_obj(range, 0x992) {
            self.doc.obj_mut(r).set(0x2af, Value::F64(from));
            self.doc.obj_mut(r).set(0x26, Value::F64(length));
        }
        if let Some(r) = self.doc.field_obj(range, 0x98c) {
            self.doc.obj_mut(r).set(0x2af, Value::F64(from));
        }
        if let Some(r) = self.doc.field_obj(range, 0x98d) {
            self.doc.obj_mut(r).set(0x2af, Value::F64(from + length));
        }
    }

    /// One audio clip as an arrangement audio clip. Its audio is the whole sample, stretched to play as long as it does
    /// in FL (warp markers at its start and end) and transposed; the clip plays the part of it the FL clip does. The
    /// sample's transients, which Bitwig found in the template's sample, are emptied for it to find this one's.
    fn add_audio_clip(
        &mut self,
        track: ObjId,
        channel: &str,
        path: &str,
        info: Option<&SampleFile>,
        a: &AudioClip,
    ) -> Result<(), String> {
        let pattern = self.audio.as_ref().map(|p| p.clip).ok_or("The Bitwig template has no audio clip")?;
        let clip = self.copy(pattern);
        {
            let c = self.doc.obj_mut(clip);
            c.set(0x2af, Value::F64(a.start));
            c.set(0x26, Value::F64(a.length));
            if a.muted || c.has(0x10f8) {
                put(c, 0x10f8, Value::Bool(a.muted.into()));
            }
        }
        if let Some(range) = self.doc.field_obj(clip, 0x2adb) {
            self.set_range(range, a.offset, a.length);
        }
        let content = self.doc.field_obj(clip, 0x2ae9).and_then(|c| self.doc.field_obj(c, 0x10e9));
        if let Some(c) = content
            && self.doc.obj(c).has(0x2d24)
        {
            self.doc.obj_mut(c).set(0x2d24, text(channel));
        }
        let audio = audio_event(&self.doc, clip).ok_or("The Bitwig template's audio clip has no audio")?;
        {
            let o = self.doc.obj_mut(audio);
            o.set(0x2af, Value::F64(0.0));
            o.set(0x26, Value::F64(a.played));
            if o.has(0x10f8) {
                put(o, 0x10f8, Value::Bool(0));
            }
            if o.has(0xc04) {
                o.set(0xc04, Value::F64(0.0));
            }
            if o.has(0xc05) {
                // Where the sample ends, in its seconds.
                o.set(0xc05, Value::F64(a.seconds));
            }
        }
        let play = self.doc.field_obj(audio, 0x288).ok_or("The Bitwig template's audio clip has no audio")?;
        // Bitwig's stretch mode that keeps pitch and length apart, as it saves a clip stretched and transposed.
        if self.doc.obj(play).has(0x4b0) {
            put(self.doc.obj_mut(play), 0x4b0, Value::U8(3));
        }
        if a.semitones.abs() < 1e-6 {
            put(self.doc.obj_mut(play), 0xc33, Value::Null);
        } else {
            let t = self.transposition(a.semitones);
            put(self.doc.obj_mut(play), 0xc33, Value::Obj(Slot::Obj(t)));
        }
        let sample = self.doc.field_obj(play, 0x20d).and_then(|s| self.doc.field_obj(s, 0x4b4));
        if let Some(s) = sample {
            self.sample_is(s, path, info);
        }
        // Warp markers: the sample's start at the clip's, its end where it ends when stretched.
        if let Some(warp) = self.doc.field_obj(play, 0x192) {
            let class = self.doc.field_list(warp, 0x21f).first().map_or(0x43, |&m| self.doc.obj(m).class);
            let mut marker = |beat: f64, second: f64| {
                let mut m = Object::new(class);
                m.set(0x2af, Value::F64(beat));
                m.set(0xa6, Value::F64(second));
                Slot::Obj(self.doc.add(m))
            };
            let markers = vec![marker(0.0, 0.0), marker(a.played, a.seconds)];
            self.doc.obj_mut(warp).set(0x21f, Value::List(markers));
        }
        if let Some(transients) = self.doc.field_obj(play, 0x10ea).and_then(|t| self.doc.field_obj(t, 0x10e9)) {
            let o = self.doc.obj_mut(transients);
            o.set(0x21f, Value::List(Vec::new()));
            if o.has(0x4406) {
                put(o, 0x4406, Value::U8(0));
            }
        }
        self.place(track, clip, content);
        Ok(())
    }

    /// An audio clip's transposition, as Bitwig saves it: one point holding the semitones.
    fn transposition(&mut self, semitones: f64) -> ObjId {
        let mut point = Object::new(0x108);
        point.set(0x2af, Value::F64(0.0));
        point.set(0x28f, Value::F64(semitones));
        point.set(0x359e, Value::U8(1));
        point.set(0xba6, Value::F64(0.0));
        point.set(0x2ebe, Value::F64(0.0));
        let point = self.doc.add(point);
        let mut expression = Object::new(0x2af);
        expression.set(0x1fd, Value::U8(0));
        expression.set(0x21f, Value::List(vec![Slot::Obj(point)]));
        self.doc.add(expression)
    }

    /// A lane in `lanes` already moves `setting`. The template's master and transport are kept with what's on them,
    /// their own lanes too, and one setting can't have two.
    fn automated(&self, lanes: Option<ObjId>, setting: ObjId) -> bool {
        lanes
            .into_iter()
            .flat_map(|l| self.doc.field_list(l, 0x4ba))
            .any(|lane| self.doc.field_obj(lane, 0x2a3).and_then(|t| self.doc.field_obj(t, 0x41)) == Some(setting))
    }

    /// The project's transport (field 0x190), which holds the song's tempo setting (0x211) and its lane, as Bitwig
    /// 6.1.6 saves them.
    fn transport(&self) -> Option<ObjId> {
        self.doc.field_obj(self.doc.root?, 0x190)
    }

    /// Gives a track the list its automation lanes go in (class 0xfa2, as Bitwig 6.1.6 saves one on every track and
    /// the master; 0xfa3 on the transport), before its mixer (or the transport's tempo) as Bitwig has it, for a
    /// template track that has none.
    fn lane_list(&mut self, track: ObjId, transport: bool) -> ObjId {
        let mut list = Object::new(if transport { 0xfa3 } else { 0xfa2 });
        list.set(0x1832, Value::Null);
        list.set(0x4ba, Value::List(Vec::new()));
        list.set(0x3f83, Value::List(Vec::new()));
        let list = self.doc.add(list);
        let o = self.doc.obj_mut(track);
        let field = mo_formats::bitwig::Field { id: 0x2dc9, value: Value::Obj(Slot::Obj(list)) };
        let before = if transport { 0x211 } else { 0x165 };
        match o.fields.iter().position(|f| f.id == before) {
            Some(at) => o.fields.insert(at, field),
            None => o.fields.push(field),
        }
        list
    }

    /// A track's device by its place in the plan, and its name, when it's converted.
    fn lane_device(&self, devices: &[Option<ObjId>], at: usize) -> Option<(ObjId, String)> {
        let device = devices.get(at).copied().flatten()?;
        Some((device, self.def(device).map(|d| self.doc.obj(d).str(0x9a).to_owned()).unwrap_or_default()))
    }

    /// Adds an automation lane to a track. `devices` are the track's devices by their place in the plan. Says why when
    /// it can't.
    fn add_lane(&mut self, track: ObjId, devices: &[Option<ObjId>], lane: &Lane) -> Option<&'static str> {
        const NO_LANE: &str = "the Bitwig template has no automation lane to copy";
        const TAKEN: &str = "the Bitwig template already automates it there";
        let lanes = self.doc.field_obj(track, 0x2dc9);
        let (pattern, set_up): (Option<ObjId>, SetUp) = match &lane.target {
            Target::Volume | Target::Pan => {
                let pan = lane.target == Target::Pan;
                let param = self
                    .doc
                    .field_obj(track, 0x165)
                    .and_then(|m| self.doc.field_obj(m, if pan { 0x1a5 } else { 0x1a4 }));
                let Some(param) = param else { return Some("this track has no fader") };
                if self.automated(lanes, param) {
                    return Some(TAKEN);
                }
                let name = if pan { "Pan" } else { "Volume" };
                (
                    self.mixer_lane,
                    Box::new(move |doc: &mut Document, target: ObjId| {
                        let o = doc.obj_mut(target);
                        put(o, 0x41, Value::Obj(Slot::Obj(param)));
                        o.set(0x133a, text(name));
                    }),
                )
            }
            Target::Tempo => {
                let Some(tempo) = self.doc.field_obj(track, 0x211) else {
                    return Some("the Bitwig template has no tempo setting");
                };
                if self.automated(lanes, tempo) {
                    return Some(TAKEN);
                }
                (
                    self.mixer_lane,
                    Box::new(move |doc: &mut Document, target: ObjId| {
                        let o = doc.obj_mut(target);
                        put(o, 0x41, Value::Obj(Slot::Obj(tempo)));
                        o.set(0x133b, text("Transport"));
                        o.set(0x133a, text("Tempo"));
                    }),
                )
            }
            Target::Plugin { device, path, name, .. } => {
                let Some((device, plugin_name)) = self.lane_device(devices, *device) else {
                    return Some("its plugin isn't converted");
                };
                let (path, name) = (path.clone(), name.clone());
                (
                    self.plugin_lane,
                    Box::new(move |doc: &mut Document, target: ObjId| {
                        let o = doc.obj_mut(target);
                        put(o, 0xec, Value::Obj(Slot::Obj(device)));
                        o.set(0xed, text(&path));
                        o.set(0x133b, text(&plugin_name));
                        o.set(0x133a, text(&name));
                    }),
                )
            }
            Target::OnOff { device } => {
                let Some((device, plugin_name)) = self.lane_device(devices, *device) else {
                    return Some("its plugin isn't converted");
                };
                (
                    self.plugin_lane,
                    Box::new(move |doc: &mut Document, target: ObjId| {
                        // As Bitwig 6.1.6 saves a device's On/Off lane: the setting "On", described as a switch.
                        let mut switch = Object::new(0xc6);
                        switch.set(0x1293, text(""));
                        switch.set(0x1b2d, Value::Bool(1));
                        let switch = doc.add(switch);
                        let o = doc.obj_mut(target);
                        put(o, 0xec, Value::Obj(Slot::Obj(device)));
                        o.set(0xed, text(ON_OFF_SETTING));
                        o.set(0x133b, text(&plugin_name));
                        o.set(0x133a, text("On/Off"));
                        put(o, 0xe92, Value::Obj(Slot::Obj(switch)));
                    }),
                )
            }
        };
        let (Some(pattern), Some(point)) = (pattern, self.point) else { return Some(NO_LANE) };
        // Checked on the pattern before anything is added, so a lane left out leaves the track as it was.
        let pattern_arrangement = self.doc.field_obj(pattern, 0x2d8c);
        if self.doc.field_obj(pattern, 0x2a3).is_none()
            || pattern_arrangement.and_then(|a| self.doc.field_obj(a, 0x35ac)).is_none()
        {
            return Some(NO_LANE);
        }
        let lanes = match lanes {
            Some(lanes) => lanes,
            None => self.lane_list(track, lane.target == Target::Tempo),
        };
        let made = self.copy(pattern);
        let Some(target) = self.doc.field_obj(made, 0x2a3) else { return Some(NO_LANE) };
        set_up(&mut self.doc, target);
        let Some(arrangement) = self.doc.field_obj(made, 0x2d8c) else { return Some(NO_LANE) };
        if self.doc.field_obj(arrangement, 0x35ac).is_none() {
            return Some(NO_LANE);
        }
        let points = match lane.target {
            Target::OnOff { .. } => {
                // A switch's lane holds a point at each change (class 0x7d: where, and on or off) in a list of its own
                // kind (class 0xa4), as Bitwig 6.1.6 saves one. Where several points share a beat, the last is where
                // the switch ends up there; an off and on again on one beat would be a click.
                let mut changes: Vec<(f64, bool)> = Vec::new();
                for p in &lane.points {
                    let on = p.value > 0.0;
                    match changes.last_mut() {
                        Some(last) if p.beat <= last.0 + SAME_BEAT => last.1 = on,
                        _ => changes.push((p.beat, on)),
                    }
                    let n = changes.len();
                    if n >= 2 && changes[n - 2].1 == changes[n - 1].1 {
                        changes.pop();
                    }
                }
                let points = changes
                    .into_iter()
                    .map(|(beat, on)| {
                        let mut o = Object::new(0x7d);
                        o.set(0x2af, Value::F64(beat));
                        o.set(0x12e, Value::Bool(on.into()));
                        Slot::Obj(self.doc.add(o))
                    })
                    .collect();
                let mut list = Object::new(0xa4);
                list.set(0x1fd, Value::U8(0));
                list.set(0x21f, Value::List(points));
                let list = self.doc.add(list);
                put(self.doc.obj_mut(arrangement), 0x35ac, Value::Obj(Slot::Obj(list)));
                None
            }
            _ => {
                let mut points = Vec::with_capacity(lane.points.len());
                for p in &lane.points {
                    let value = if lane.target == Target::Volume { p.value.cbrt() } else { p.value };
                    let copy = self.copy(point);
                    let o = self.doc.obj_mut(copy);
                    o.set(0x2af, Value::F64(p.beat));
                    o.set(0x28f, Value::F64(value));
                    if o.has(0x359e) {
                        put(o, 0x359e, Value::U8(1));
                    }
                    if o.has(0xba6) {
                        // A straight line to the next point.
                        o.set(0xba6, Value::F64(0.0));
                    }
                    points.push(Slot::Obj(copy));
                }
                Some(points)
            }
        };
        if let Some(points) = points
            && let Some(list) = self.doc.field_obj(arrangement, 0x35ac)
        {
            self.doc.obj_mut(list).set(0x21f, Value::List(points));
        }
        push(self.doc.obj_mut(lanes), 0x4ba, Slot::Obj(made));

        // The plugin also lists the setting, as Bitwig saves an automated one.
        if let (Target::Plugin { device, id, .. }, Some(setting)) = (&lane.target, self.setting)
            && let Some(Some(device)) = devices.get(*device).copied()
            && let Some(def) = self.def(device)
        {
            let listed =
                self.doc.field_list(def, 0x1318).iter().any(|&o| self.doc.obj(o).num(0x1319) == Some(*id as f64));
            if !listed {
                let copy = self.copy(setting);
                let o = self.doc.obj_mut(copy);
                put(o, 0x1319, int((*id).into()));
                if o.has(0x131a)
                    && let Some(first) = lane.points.first()
                {
                    o.set(0x131a, Value::F64(first.value));
                }
                push(self.doc.obj_mut(def), 0x1318, Slot::Obj(copy));
            }
        }
        None
    }

    /// Puts the new tracks in the project and writes it, with the plugin settings in its zip.
    fn finish(mut self, tracks: &[ObjId], tempo: f64, states: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
        self.doc.obj_mut(self.group).set(0x4de, Value::List(tracks.iter().map(|&t| Slot::Obj(t)).collect()));
        let root = self.doc.root.ok_or("no project")?;
        for o in reachable(&self.doc, root) {
            if self.doc.obj(o).str(0x2bd) == "TEMPO" && self.doc.obj(o).has(0x2c8) {
                self.doc.obj_mut(o).set(0x2c8, Value::F64(tempo));
            }
        }

        // The browser's summary of the project lists its tracks by name.
        if let Some(summary) = self.summary(tracks) {
            let at = self.doc.meta.iter().rposition(|(k, _)| k.as_str() == "structure");
            if let Some((_, v)) = at.and_then(|i| self.doc.meta.get_mut(i)) {
                *v = Value::Blob(Box::new(Blob::new(&summary)));
            }
        }

        // The template's own plugins still in the project (on its master) keep their settings from its zip.
        let made: HashSet<&str> = states.iter().map(|(n, _)| n.as_str()).collect();
        let kept: HashSet<String> = reachable(&self.doc, root)
            .into_iter()
            .map(|o| self.doc.obj(o).str(0xbe5).to_owned())
            .filter(|n| !n.is_empty() && !made.contains(n.as_str()))
            .collect();
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut any = !states.is_empty();
        if !kept.is_empty()
            && let Some(raw) = self.doc.zip()
            && let Ok(mut template) = zip::ZipArchive::new(Cursor::new(raw))
        {
            for i in 0..template.len() {
                let Ok(mut entry) = template.by_index(i) else { continue };
                let Ok(name) = entry.name().map(|n| n.to_string()) else { continue };
                if name.strip_prefix(STATES).is_some_and(|n| kept.contains(n)) {
                    let mut data = Vec::new();
                    entry.read_to_end(&mut data).map_err(|e| e.to_string())?;
                    zip.start_file(name, options).map_err(|e| e.to_string())?;
                    zip.write_all(&data).map_err(|e| e.to_string())?;
                    any = true;
                }
            }
        }
        for (name, data) in states {
            zip.start_file(format!("{STATES}{name}"), options).map_err(|e| e.to_string())?;
            zip.write_all(data).map_err(|e| e.to_string())?;
        }
        let zip = zip.finish().map_err(|e| e.to_string())?.into_inner();
        bitwig::encode(&self.doc, Some(if any { &zip } else { &[] }))
    }

    /// The project's summary (metadata "structure", a document of its own) listing the new tracks, or `None` when the
    /// template's can't be read, so it's left as it was.
    fn summary(&self, tracks: &[ObjId]) -> Option<Vec<u8>> {
        let Some(Value::Blob(blob)) = self.doc.meta("structure") else { return None };
        let mut summary = bitwig::decode(blob.bytes()).ok()?;
        if summary.body_error.is_some() {
            return None;
        }
        let project = summary.field_obj(summary.root?, 0xad2)?;
        let entries = summary.field_list(project, 0xace);
        let first_of = |audio: bool| entries.iter().copied().find(|&e| (summary.obj(e).class == AUDIO_ENTRY) == audio);
        let track_entry = first_of(false).or(entries.first().copied())?;
        // Audio tracks have their own kind of entry; the template's, if it lists one.
        let audio_entry = first_of(true).unwrap_or(track_entry);
        summary.obj_mut(project).set(0xace, Value::List(Vec::new()));
        summary.obj_mut(project).set(0xacf, Value::List(Vec::new()));
        let copier = Copier::new(&summary, None);
        if copier.shares(track_entry) || copier.shares(audio_entry) {
            return None;
        }
        let mut list = Vec::with_capacity(tracks.len());
        for &t in tracks {
            let e = copier.copy(&mut summary, if self.audio_tracks.contains(&t) { audio_entry } else { track_entry });
            summary.obj_mut(e).set(0xad1, text(self.doc.obj(t).str(0x15b)));
            list.push(Slot::Obj(e));
        }
        summary.obj_mut(project).set(0xace, Value::List(list));
        bitwig::encode(&summary, None).ok()
    }
}

fn chain_of(doc: &Document, track: ObjId) -> Vec<ObjId> {
    doc.field_obj(track, 0x164).map(|c| doc.field_list(c, 0x144)).unwrap_or_default()
}

fn set_chain_of(doc: &mut Document, track: ObjId, devices: &[ObjId]) {
    if let Some(c) = doc.field_obj(track, 0x164) {
        doc.obj_mut(c).set(0x144, Value::List(devices.iter().map(|&d| Slot::Obj(d)).collect()));
    }
}

fn clips_of(doc: &Document, track: ObjId) -> Vec<ObjId> {
    doc.field_obj(track, 0x2bf8)
        .and_then(|a| doc.field_obj(a, 0x2d8e))
        .map(|a| doc.field_list(a, 0x21f))
        .unwrap_or_default()
}

fn note_content(doc: &Document, clip: ObjId) -> Option<ObjId> {
    let c = doc.field_obj(clip, 0x2ae9)?;
    let c = doc.field_obj(c, 0x10e9)?;
    doc.field_obj(c, 0x49c)
}

fn key_lanes(doc: &Document, clip: ObjId) -> Vec<ObjId> {
    note_content(doc, clip).map(|c| doc.field_list(c, 0x18cb)).unwrap_or_default()
}

/// An audio clip's audio: its one event, which plays the sample.
fn audio_event(doc: &Document, clip: ObjId) -> Option<ObjId> {
    if doc.obj(clip).class != AUDIO_CLIP_CLASS {
        return None;
    }
    doc.field_list(*key_lanes(doc, clip).first()?, 0x21f).first().copied()
}

fn is_audio_clip(doc: &Document, clip: ObjId) -> bool {
    let sample = audio_event(doc, clip)
        .and_then(|a| doc.field_obj(a, 0x288))
        .and_then(|p| doc.field_obj(p, 0x20d))
        .and_then(|s| doc.field_obj(s, 0x4b4));
    sample.is_some_and(|s| doc.obj(s).has(0x129e))
}

/// Every object reachable from `start`, each once, depth first in field order.
fn reachable(doc: &Document, start: ObjId) -> Vec<ObjId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut todo = vec![start];
    while let Some(o) = todo.pop() {
        if !seen.insert(o) {
            continue;
        }
        out.push(o);
        let next: Vec<ObjId> =
            doc.obj(o).fields.iter().flat_map(|f| f.value.targets()).filter_map(|s| doc.resolve(s)).collect();
        todo.extend(next.into_iter().rev());
    }
    out
}

/// Changes a field to exactly this value (and type), or adds it: for where Bitwig's own type must be written, not the
/// type the field had.
fn put(o: &mut Object, id: u32, value: Value) {
    match o.fields.iter_mut().find(|f| f.id == id) {
        Some(f) => f.value = value,
        None => o.set(id, value),
    }
}

/// Adds a slot to a list field, making the list if there's none.
fn push(o: &mut Object, id: u32, slot: Slot) {
    match o.fields.iter_mut().find(|f| f.id == id) {
        Some(f) => match &mut f.value {
            Value::List(l) => l.push(slot),
            v => *v = Value::List(vec![slot]),
        },
        None => o.set(id, Value::List(vec![slot])),
    }
}

fn text(s: &str) -> Value {
    Value::Str(Text::new(s))
}

/// The smallest integer type that holds a value, as Bitwig writes integers.
fn int(v: i64) -> Value {
    if let Ok(b) = u8::try_from(v) {
        Value::U8(b)
    } else if let Ok(s) = i16::try_from(v) {
        Value::I16(s)
    } else if let Ok(i) = i32::try_from(v) {
        Value::I32(i)
    } else {
        Value::I64(v)
    }
}

/// A field's value as text: a VST2 id is a number, a VST3's text.
fn value_text(v: &Value) -> String {
    match v {
        Value::Str(t) => t.as_str().to_owned(),
        Value::U8(n) | Value::Bool(n) => n.to_string(),
        Value::I16(n) => n.to_string(),
        Value::I32(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        _ => String::new(),
    }
}

fn uuid_bytes(s: &str) -> [u8; 16] {
    let hex: Vec<u8> = s.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        let pair = hex.get(i * 2..i * 2 + 2).and_then(|p| std::str::from_utf8(p).ok());
        *b = pair.and_then(|p| u8::from_str_radix(p, 16).ok()).unwrap_or(0);
    }
    out
}

fn uuid_text(b: &[u8; 16]) -> String {
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let part = |r: std::ops::Range<usize>| hex.get(r).unwrap_or_default().to_owned();
    format!("{}-{}-{}-{}-{}", part(0..8), part(8..12), part(12..16), part(16..20), part(20..32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_read_and_write_as_bitwig_spells_them() {
        let b = uuid_bytes(SAMPLER_ID);
        assert_eq!(uuid_text(&b), SAMPLER_ID);
        assert_eq!(int(200), Value::U8(200));
        assert_eq!(int(-2), Value::I16(-2));
        assert_eq!(int(1_144_278_322), Value::I32(1_144_278_322));
    }
}
