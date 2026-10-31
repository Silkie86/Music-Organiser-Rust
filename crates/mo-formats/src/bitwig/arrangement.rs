//! A Bitwig project's arrangement as Bitwig shows it: its tracks (groups indented) and the clips on their timelines;
//! when nothing is on the timeline, the clip launcher's scenes one after another. Ported from the C# app's
//! `Parsers/BitwigArrangement.cs`, as mapped from Solomon's projects (Bitwig 5.1 to 6.1).

use std::collections::HashSet;

use super::format::{Document, ObjId, Slot, Value};
use super::reader::{
    CLS_BITWIG_DEVICE, CLS_CLIP, CLS_NOTE, CLS_TEMPO, CLS_VST2_PLUGIN, CLS_VST3_PLUGIN, F_DEVICE_CATEGORY,
    F_DEVICE_NAME, F_DEVICE_PRESET, F_DURATION, F_FILE_PATH, F_GROUP_TRACKS, F_NOTES, F_PITCH, F_PROJECT_GROUP,
    F_START, F_TEMPO, F_VELOCITY, bitwig_kind, field_obj, is_instrument_category, track_names,
};
use crate::PluginKind;
use crate::arrangement::{Arrangement, AudioPart, LaneKind, PreviewClip, PreviewLane, PreviewNote};
use crate::velocity;

pub const VIEW: &str = "Bitwig arranger";

/// A track's colour: four floats, red, green, blue and alpha from 0 to 1.
const F_TRACK_COLOUR: u32 = 0x54c;
const F_TRACK_LANES: u32 = 0x2bf8;
const F_ARRANGER_LANE: u32 = 0x2d8e;
const F_LAUNCHER_LANE: u32 = 0x2d8f;
const F_LAUNCHER_SLOTS: u32 = 0x231;
/// The list of things on a lane: clips on a track's lane, notes on a key lane, events on an audio lane.
const F_ITEMS: u32 = F_NOTES;
const F_MUTED: u32 = 0x10f8;
const F_DEVICE_CHAIN: u32 = 0x164;
const F_CLIP_CONTENT: u32 = 0x2ae9;
const F_CONTENT_OBJECT: u32 = 0x10e9;
const F_CONTENT_NAME: u32 = 0x2d24;
const F_NOTE_CONTENT: u32 = 0x49c;
const F_KEY_LANES: u32 = 0x18cb;
const F_PLAY_RANGE: u32 = 0x2adb;
const F_LOOP_RANGE: u32 = 0x992;
const F_LOOP_ON: u32 = 0x991;
const F_PLAY_START: u32 = 0x98c;
const CLS_AUDIO_CLIP: u32 = 0xee;
// An audio event: what it plays (0x288 → 0x20d → 0x4b4, a sample whose file is 0x129e → 0xd3b), the seconds of the
// file it starts and ends at, and warp markers (0x192: points of a beat 0x2af and a second 0xa6 of the file).
const F_AUDIO_PLAY: u32 = 0x288;
const F_AUDIO_SOURCE: u32 = 0x20d;
const F_AUDIO_SAMPLE: u32 = 0x4b4;
const F_SAMPLE_FILE: u32 = 0x129e;
const F_FILE_REF: u32 = 0xd3b;
const F_SAMPLE_START: u32 = 0xc04;
const F_SAMPLE_END: u32 = 0xc05;
const F_WARP: u32 = 0x192;
const F_WARP_SECOND: u32 = 0xa6;

/// The project's arrangement as Bitwig shows it. A project that didn't decode cleanly says so instead.
pub fn read(doc: &Document) -> Arrangement {
    let Some(root) = doc.root.filter(|_| doc.body_error.is_none()) else {
        return crate::arrangement::unreadable(VIEW, "This project");
    };
    let names = track_names(doc);
    let tempo = doc
        .objects
        .iter()
        .filter(|o| o.class == CLS_TEMPO)
        .filter_map(|o| o.num(F_TEMPO))
        .find(|t| (10.0..=999.0).contains(t))
        .unwrap_or(120.0);

    let mut lanes = Vec::new();
    // Each track's launcher slots, the first clip in each (read only when nothing is on the timeline).
    let mut launcher: Vec<Vec<Option<ObjId>>> = Vec::new();
    let mut seen = HashSet::new();
    // The tracks left to walk in each group, last first, with how deep the group is.
    let mut stack: Vec<(Vec<ObjId>, u32)> = Vec::new();
    if let Some(project) = field_obj(doc, root, F_PROJECT_GROUP) {
        stack.push((doc.field_list(project, F_GROUP_TRACKS).into_iter().rev().collect(), 0));
    }
    while let Some((left, depth)) = stack.last_mut() {
        let depth = *depth;
        let Some(track) = left.pop() else {
            stack.pop();
            continue;
        };
        // A group listed inside itself would go round for ever.
        if !seen.insert(track) {
            continue;
        }
        let children = doc.field_list(track, F_GROUP_TRACKS);
        let clips: Vec<PreviewClip> = field_obj(doc, track, F_TRACK_LANES)
            .and_then(|l| field_obj(doc, l, F_ARRANGER_LANE))
            .map(|l| doc.field_list(l, F_ITEMS))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| clip(doc, c))
            .collect();
        let kind = if !children.is_empty() {
            LaneKind::Group
        } else if clips.iter().any(|c| !c.audio.is_empty()) {
            LaneKind::Audio
        } else {
            LaneKind::Instrument
        };
        let mut lane = PreviewLane::new(names.get(track).unwrap_or("Track"), kind);
        lane.detail = first_device(doc, track);
        lane.colour = colour(doc.obj(track).get(F_TRACK_COLOUR));
        lane.depth = depth;
        lane.clips = clips;
        lanes.push(lane);
        launcher.push(
            launcher_slots(doc, track)
                .into_iter()
                .map(|slot| descendants(doc, slot, false, |_| false).find(|&o| is_clip(doc, o)))
                .collect(),
        );
        if !children.is_empty() {
            stack.push((children.into_iter().rev().collect(), depth + 1));
        }
    }

    let mut notes = Vec::new();
    if !lanes.is_empty() && lanes.iter().all(|l| l.clips.is_empty()) {
        // Nothing on the timeline: lay the clip launcher's scenes end to end instead, each as long as its longest clip.
        let slots: Vec<Vec<Option<PreviewClip>>> =
            launcher.iter().map(|row| row.iter().map(|c| c.and_then(|c| clip(doc, c))).collect()).collect();
        let scenes = slots.iter().map(Vec::len).max().unwrap_or(0);
        let mut at = 0.0;
        let mut placed: Vec<Vec<PreviewClip>> = vec![Vec::new(); lanes.len()];
        for scene in 0..scenes {
            let in_scene: Vec<(usize, &PreviewClip)> =
                slots.iter().enumerate().filter_map(|(i, row)| row.get(scene)?.as_ref().map(|c| (i, c))).collect();
            if in_scene.is_empty() {
                continue;
            }
            for (lane, c) in &in_scene {
                if let Some(p) = placed.get_mut(*lane) {
                    p.push(PreviewClip { start: at, ..(*c).clone() });
                }
            }
            at += in_scene.iter().map(|(_, c)| c.length).fold(f64::MIN, f64::max);
        }
        if at > 0.0 {
            for (lane, clips) in lanes.iter_mut().zip(placed) {
                lane.clips = clips;
            }
            notes.push(
                "This project's clips are in the clip launcher, so its scenes are shown one after another.".into(),
            );
        } else {
            notes.push("Nothing is on the arrangement timeline or in the clip launcher yet.".into());
        }
    }
    Arrangement { view: VIEW, tempo, beats_per_bar: 4, lanes, markers: Vec::new(), notes }
}

/// A track's clip launcher slots, one per scene (track → 0x2bf8 → 0x2d8f → 0x231).
fn launcher_slots(doc: &Document, track: ObjId) -> Vec<ObjId> {
    field_obj(doc, track, F_TRACK_LANES)
        .and_then(|l| field_obj(doc, l, F_LAUNCHER_LANE))
        .map(|l| doc.field_list(l, F_LAUNCHER_SLOTS))
        .unwrap_or_default()
}

fn is_clip(doc: &Document, o: ObjId) -> bool {
    let o = doc.obj(o);
    matches!(o.class, CLS_CLIP | CLS_AUDIO_CLIP) && o.has(F_START) && o.has(F_DURATION) && o.has(F_CLIP_CONTENT)
}

/// Everything inside an object, depth first, in the order it's written: what's defined there, and with `refs` also
/// what it refers to (a track's devices are often references). `stop` leaves an object and what's in it out.
fn descendants<'a>(
    doc: &'a Document,
    root: ObjId,
    refs: bool,
    stop: impl Fn(ObjId) -> bool + 'a,
) -> impl Iterator<Item = ObjId> + 'a {
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    std::iter::from_fn(move || {
        while let Some(o) = stack.pop() {
            if !seen.insert(o) {
                continue;
            }
            if o != root && stop(o) {
                continue;
            }
            let follow = |s: Slot| match s {
                Slot::Obj(id) => Some(id),
                Slot::Ref(_) if refs => doc.resolve(s),
                _ => None,
            };
            for f in doc.obj(o).fields.iter().rev() {
                match &f.value {
                    Value::Obj(s) => stack.extend(follow(*s)),
                    Value::BareRef(s) if refs => stack.extend(doc.resolve(*s)),
                    Value::List(l) => stack.extend(l.iter().rev().filter_map(|s| follow(*s))),
                    _ => {}
                }
            }
            if o != root {
                return Some(o);
            }
        }
        None
    })
}

/// What plays the track: its instrument (a device and its preset), else its first device.
fn first_device(doc: &Document, track: ObjId) -> String {
    // The device chain only; clips and sub-tracks hold no devices of this track's.
    let Some(chain) = field_obj(doc, track, F_DEVICE_CHAIN) else { return String::new() };
    // References are followed, but never into another track (a sidechain or modulator can point at one).
    let other_track = |o: ObjId| doc.obj(o).has(F_TRACK_LANES) || doc.obj(o).has(F_GROUP_TRACKS);
    let devices: Vec<ObjId> = descendants(doc, chain, true, other_track)
        .filter(|&o| {
            let d = doc.obj(o);
            matches!(d.class, CLS_BITWIG_DEVICE | CLS_VST3_PLUGIN | CLS_VST2_PLUGIN) && !d.str(F_DEVICE_NAME).is_empty()
        })
        .collect();
    let kind = |o: ObjId| bitwig_kind(doc.obj(o).str(F_DEVICE_NAME), doc.obj(o).str(F_DEVICE_CATEGORY));
    let device = devices
        .iter()
        .find(|&&d| kind(d) == PluginKind::Instrument || is_instrument_category(doc.obj(d).str(F_DEVICE_CATEGORY)))
        .or_else(|| devices.iter().find(|&&d| kind(d) != PluginKind::NoteEffect))
        .or(devices.first());
    let Some(&device) = device else { return String::new() };
    let (name, preset) = (doc.obj(device).str(F_DEVICE_NAME), doc.obj(device).str(F_DEVICE_PRESET));
    if !preset.is_empty() && preset != name { format!("{name} · {preset}") } else { name.to_owned() }
}

fn colour(value: Option<&Value>) -> Option<u32> {
    let channel = |c: f32| ((c as f64).clamp(0.0, 1.0) * 255.0).round() as u32;
    match value? {
        Value::Color([r, g, b, _]) => Some(channel(*r) << 16 | channel(*g) << 8 | channel(*b)),
        Value::Floats(c) => match c.as_slice() {
            [r, g, b, ..] => Some(channel(*r) << 16 | channel(*g) << 8 | channel(*b)),
            _ => None,
        },
        _ => None,
    }
}

/// An arrangement clip: where it is, and what of its content plays. The content has its own timeline; the clip plays
/// it from its play start, and when looping is on, goes back to the loop range's start each time it reaches its end.
fn clip(doc: &Document, clip: ObjId) -> Option<PreviewClip> {
    let c = doc.obj(clip);
    let (start, length) = (c.num(F_START)?, c.num(F_DURATION)?);
    if length.is_nan() || length <= 0.0 {
        return None;
    }
    let muted = c.num(F_MUTED).is_some_and(|m| m > 0.0);
    let content = field_obj(doc, clip, F_CLIP_CONTENT).and_then(|o| field_obj(doc, o, F_CONTENT_OBJECT));
    let range = field_obj(doc, clip, F_PLAY_RANGE);
    let loop_range = range.and_then(|r| field_obj(doc, r, F_LOOP_RANGE));
    let num = |o: Option<ObjId>, field: u32| o.and_then(|o| doc.obj(o).num(field));
    let offset =
        num(range.and_then(|r| field_obj(doc, r, F_PLAY_START)), F_START).or(num(loop_range, F_START)).unwrap_or(0.0);
    let loop_start = num(loop_range, F_START).unwrap_or(0.0);
    let loop_end = loop_start + num(loop_range, F_DURATION).unwrap_or(0.0);
    let looping = num(range, F_LOOP_ON).is_some_and(|v| v > 0.0) && loop_end > loop_start;
    let key_lanes = content.and_then(|o| field_obj(doc, o, F_NOTE_CONTENT)).map(|o| doc.field_list(o, F_KEY_LANES));
    let key_lanes = key_lanes.unwrap_or_default();
    let name = content.map(|o| doc.obj(o).str(F_CONTENT_NAME)).unwrap_or_default();

    // Where a moment of the content falls in the clip: each pass of the loop after the first starts at the loop start.
    let placements = |at: f64| -> Vec<f64> {
        let mut out = Vec::new();
        if !looping {
            if at >= offset && at < offset + length {
                out.push(at - offset);
            }
            return out;
        }
        let pass_length = loop_end - loop_start;
        if at >= offset && at < loop_end && at - offset < length {
            out.push(at - offset);
        }
        if at < loop_start || at >= loop_end {
            return out;
        }
        let mut pass = loop_end - offset + (at - loop_start);
        while pass < length {
            out.push(pass);
            pass += pass_length;
        }
        out
    };

    if c.class == CLS_AUDIO_CLIP {
        let mut parts = Vec::new();
        for e in key_lanes.iter().flat_map(|&l| doc.field_list(l, F_ITEMS)) {
            let play = field_obj(doc, e, F_AUDIO_PLAY);
            let file = [F_AUDIO_SOURCE, F_AUDIO_SAMPLE, F_SAMPLE_FILE, F_FILE_REF]
                .into_iter()
                .fold(play, |o, f| o.and_then(|o| field_obj(doc, o, f)))
                .map(|o| doc.obj(o).str(F_FILE_PATH))
                .unwrap_or_default();
            let ev = doc.obj(e);
            let e_start = ev.num(F_START).unwrap_or(0.0);
            let e_length = ev.num(F_DURATION).unwrap_or(0.0);
            if file.is_empty() || e_length.is_nan() || e_length <= 0.0 {
                continue;
            }
            let from = ev.num(F_SAMPLE_START).unwrap_or(0.0);
            let to = ev.num(F_SAMPLE_END).unwrap_or(0.0);
            let warps: Vec<(f64, f64)> = play
                .and_then(|p| field_obj(doc, p, F_WARP))
                .map(|w| doc.field_list(w, F_ITEMS))
                .unwrap_or_default()
                .into_iter()
                .map(|w| (doc.obj(w).num(F_START).unwrap_or(0.0), doc.obj(w).num(F_WARP_SECOND).unwrap_or(0.0)))
                .collect();
            let rate = match (warps.first(), warps.last()) {
                (Some(a), Some(b)) if warps.len() >= 2 && b.0 > a.0 => Some((b.1 - a.1) / (b.0 - a.0)),
                _ if to > from => Some((to - from) / e_length),
                _ => None,
            };
            // The part of the event that shows: from where the clip's window starts to where it (or the clip) ends.
            let shown_from = e_start.max(offset);
            let shown_to = (e_start + e_length).min(offset + length);
            if shown_to <= shown_from {
                continue;
            }
            parts.push(AudioPart {
                path: file.to_owned(),
                start: shown_from - offset,
                length: shown_to - shown_from,
                from: from + (shown_from - e_start) * rate.unwrap_or(0.0),
                seconds_per_beat: rate,
                whole_file_beats: None,
                reversed: false,
            });
        }
        let name = if name.is_empty() {
            parts.first().map(|p| crate::paths::file_name(&p.path).to_owned()).unwrap_or_default()
        } else {
            name.to_owned()
        };
        return Some(PreviewClip { name, start, length, muted, audio: parts, ..PreviewClip::default() });
    }

    let mut notes = Vec::new();
    for &lane in &key_lanes {
        let Some(key) = doc.obj(lane).num(F_PITCH) else { continue };
        for n in doc.field_list(lane, F_ITEMS) {
            let n = doc.obj(n);
            if n.class != CLS_NOTE {
                continue;
            }
            let at = n.num(F_START).unwrap_or(0.0);
            let duration = n.num(F_DURATION).unwrap_or(0.25);
            let velocity = velocity::from_unit(n.num(F_VELOCITY).unwrap_or(0.8)).max(velocity::from_7(1));
            for placed in placements(at) {
                notes.push(PreviewNote {
                    start: placed,
                    length: duration.min(length - placed),
                    key: (key as i32).clamp(0, 127) as u8,
                    velocity,
                    muted: n.num(F_MUTED).is_some_and(|m| m > 0.0),
                });
            }
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    Some(PreviewClip { name: name.to_owned(), start, length, muted, notes, ..PreviewClip::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_from_floats() {
        assert_eq!(colour(Some(&Value::Color([1.0, 0.5, 0.0, 1.0]))), Some(0xFF_80_00));
        assert_eq!(colour(Some(&Value::Floats(vec![2.0, -1.0, 0.2]))), Some(0xFF_00_33));
        assert_eq!(colour(Some(&Value::Floats(vec![1.0]))), None);
        assert_eq!(colour(None), None);
    }
}
