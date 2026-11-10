//! A Bitwig project's arrangement as Bitwig shows it: its tracks (groups indented) and the clips on their timelines,
//! and the clip launcher's scenes one after another (shown first when nothing is on the timeline). Ported from the C# app's
//! `Parsers/BitwigArrangement.cs`, as mapped from Solomon's projects (Bitwig 2 to 6.1; the
//! way a track holds its clips changed in Bitwig 6).

use std::collections::HashSet;

use super::format::{Document, ObjId, Slot, Value};
use super::reader::{
    CLS_BITWIG_DEVICE, CLS_CLIP, CLS_NOTE, CLS_TEMPO, CLS_VST2_PLUGIN, CLS_VST3_PLUGIN, F_DEVICE_CATEGORY,
    F_DEVICE_NAME, F_DEVICE_PRESET, F_DURATION, F_FILE_PATH, F_GROUP_TRACKS, F_NOTES, F_PITCH, F_PROJECT_GROUP,
    F_START, F_TEMPO, F_VELOCITY, bitwig_kind, field_obj, is_instrument_category, track_names,
};
use crate::PluginKind;
use crate::arrangement::{
    Arrangement, AudioPart, LaneKind, MOST_BEATS, MOST_NOTES, NoteBudget, PreviewClip, PreviewLane, PreviewMarker,
    PreviewNote, Stretch, TOO_MANY_NOTES,
};
use crate::velocity;

pub const VIEW: &str = "Bitwig arranger";
pub const SCENES_VIEW: &str = "Bitwig scenes";

/// The most times a looping audio clip's loop is drawn (a tiny loop in a long clip would otherwise be drawn for ever).
const MOST_PASSES: usize = 4096;
/// The most audio parts one clip draws (each pass of each piece of audio is one).
const MOST_AUDIO_PARTS: usize = 20_000;

/// A track's colour: four floats, red, green, blue and alpha from 0 to 1.
const F_TRACK_COLOUR: u32 = 0x54c;
const F_TRACK_LANES: u32 = 0x2bf8;
const F_ARRANGER_LANE: u32 = 0x2d8e;
const F_LAUNCHER_LANE: u32 = 0x2d8f;
const F_LAUNCHER_SLOTS: u32 = 0x231;
/// A launcher slot's clip (defined there, or referred to when it's defined at its first use elsewhere).
const F_SLOT_CLIP: u32 = 0x240;
// The scenes (Bitwig 6.1): root → 0x190 → 0x352f → 0x2ddb → 0x293, a list of scenes (class 0x6d) named in 0x109.
const F_SCENES_HOLDER: u32 = 0x190;
const F_SCENES_LANES: u32 = 0x352f;
const F_SCENES_LIST: u32 = 0x2ddb;
const F_SCENES: u32 = 0x293;
const F_SCENE_NAME: u32 = 0x109;
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
// Before Bitwig 6 a track held its timeline in 0x15d → 0x238 and its launcher in 0x15e (its slots in 0x231), and a
// clip held its content in 0x288 (the same object as 6's 0x2ae9 → 0x10e9), its name in 0x286 and its play range in
// the content's 0x98f. Seen in Solomon's saves from Bitwig 2 to 5.2.
const F_OLD_TIMELINE: u32 = 0x15d;
const F_OLD_ARRANGER_LANE: u32 = 0x238;
const F_OLD_LAUNCHER: u32 = 0x15e;
const F_OLD_CLIP_CONTENT: u32 = 0x288;
const F_OLD_CLIP_NAME: u32 = 0x286;
const F_OLD_PLAY_RANGE: u32 = 0x98f;
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
// What each audio event's playback (0x288) says about how it plays, as Solomon's recipe saves showed (6.1.7): 0x4b0 its
// stretch mode, 0x10ad set when it's reversed, and its transpose (0xc33) and gain (0xc34) as expressions whose first
// point (0x21f) has the value in 0x28f: semitones, and the gain as the cube root of its level (-6 dB is 0.794).
const F_STRETCH_MODE: u32 = 0x4b0;
const F_REVERSED: u32 = 0x10ad;
const F_TRANSPOSE: u32 = 0xc33;
const F_GAIN: u32 = 0xc34;
const F_EXPRESSION_VALUE: u32 = 0x28f;
// An audio clip's fade in and out (0x1947, 0x1948), in beats as far as the saves show.
const F_FADE_IN: u32 = 0x1947;
const F_FADE_OUT: u32 = 0x1948;

/// The project's arrangement as Bitwig shows it: its arranger timeline, and its clip launcher's scenes one after
/// another. The scenes come first when nothing is on the timeline. A project that didn't decode cleanly says so instead.
pub fn read(doc: &Document) -> Vec<Arrangement> {
    let Some(root) = doc.root.filter(|_| doc.body_error.is_none()) else {
        return vec![crate::arrangement::unreadable(VIEW, "This project")];
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
    let mut budget = NoteBudget::new();
    // Each track's launcher slots, the clip in each (for the scenes view).
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
        let clips: Vec<PreviewClip> = track_lanes(doc, track)
            .0
            .map(|l| doc.field_list(l, F_ITEMS))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| clip(doc, c, &mut budget, None))
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
        launcher.push(launcher_slots(doc, track).into_iter().map(|slot| slot_clip(doc, slot)).collect());
        if !children.is_empty() {
            stack.push((children.into_iter().rev().collect(), depth + 1));
        }
    }

    // The scenes view has a note budget of its own, so a busy arranger can't leave it empty.
    let scenes = scenes(doc, root, tempo, &lanes, &launcher);
    let mut notes = Vec::new();
    if budget.ran_out {
        notes.push(TOO_MANY_NOTES.to_owned());
    }
    let timeline_empty = lanes.iter().all(|l| l.clips.is_empty());
    let mut arranger = Arrangement { view: VIEW, tempo, beats_per_bar: 4, lanes, markers: Vec::new(), notes };
    match scenes {
        None => {
            if timeline_empty && !arranger.lanes.is_empty() {
                arranger.notes.push("Nothing is on the arrangement timeline or in the clip launcher yet.".into());
            }
            vec![arranger]
        }
        Some(scenes) => {
            if timeline_empty {
                arranger
                    .notes
                    .push("Nothing is on the arrangement timeline; the clips are in the clip launcher.".into());
                vec![scenes, arranger]
            } else {
                vec![arranger, scenes]
            }
        }
    }
}

/// The clip launcher's scenes one after another, on the same tracks as the arranger: each scene as long as its
/// longest clip (a looping clip's loop), looping clips repeating to fill it, and a marker naming each. None when the
/// launcher is empty.
fn scenes(
    doc: &Document,
    root: ObjId,
    tempo: f64,
    lanes: &[PreviewLane],
    launcher: &[Vec<Option<ObjId>>],
) -> Option<Arrangement> {
    // Nothing in the launcher: no scenes view, and no scene names walked for it.
    if launcher.iter().flatten().all(Option::is_none) {
        return None;
    }
    let mut budget = NoteBudget::new();
    let names = scene_names(doc, root);
    let count = launcher.iter().map(Vec::len).max().unwrap_or(0);
    // The arranger's tracks without their clips (copying them only to drop them would cost a big project dear).
    let mut placed: Vec<PreviewLane> = lanes
        .iter()
        // Every field named, so one added to PreviewLane later must be thought about here too.
        .map(|l| PreviewLane {
            name: l.name.clone(),
            kind: l.kind,
            detail: l.detail.clone(),
            colour: l.colour,
            depth: l.depth,
            muted: l.muted,
            clips: Vec::new(),
        })
        .collect();
    let mut markers = Vec::new();
    let mut at = 0.0;
    for scene in 0..count {
        let in_scene: Vec<(usize, ObjId, f64)> = launcher
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                let c = (*row.get(scene)?)?;
                Some((i, c, launcher_length(doc, c)?))
            })
            .collect();
        let Some(length) = in_scene.iter().map(|&(_, _, l)| l).reduce(f64::max) else { continue };
        for &(lane, c, _) in &in_scene {
            let Some(mut clip) = clip(doc, c, &mut budget, Some(length)) else { continue };
            clip.start = at;
            clip.length = clip.length.min(length);
            if let Some(l) = placed.get_mut(lane) {
                l.clips.push(clip);
            }
        }
        let name =
            names.get(scene).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| format!("Scene {}", scene + 1));
        markers.push(PreviewMarker { beat: at, name });
        at += length;
    }
    if markers.is_empty() {
        return None;
    }
    for lane in &mut placed {
        if lane.kind != LaneKind::Group {
            let audio = lane.clips.iter().any(|c| !c.audio.is_empty());
            lane.kind = if audio { LaneKind::Audio } else { LaneKind::Instrument };
        }
    }
    let mut notes = vec![
        "The clip launcher's scenes one after another: each plays as long as its longest clip, and looping clips \
         repeat to fill it."
            .to_owned(),
    ];
    if budget.ran_out {
        notes.push(TOO_MANY_NOTES.to_owned());
    }
    Some(Arrangement { view: SCENES_VIEW, tempo, beats_per_bar: 4, lanes: placed, markers, notes })
}

/// The clip in a launcher slot (the slot's 0x240). Bitwig 6 defines a clip at its first use, which can be the track's
/// 0x15f, so the slot may only refer to it.
fn slot_clip(doc: &Document, slot: ObjId) -> Option<ObjId> {
    field_obj(doc, slot, F_SLOT_CLIP)
        .filter(|&c| is_clip(doc, c))
        .or_else(|| descendants(doc, slot, false, |_| false).find(|&o| is_clip(doc, o)))
}

/// How long a launcher clip plays once round: when it loops, from its start marker to its loop's end (at least one
/// whole loop), else its own length.
fn launcher_length(doc: &Document, clip: ObjId) -> Option<f64> {
    let length = doc.obj(clip).num(F_DURATION).filter(|l| l.is_finite() && *l > 0.0)?;
    let t = timing(doc, clip);
    let once = if t.looping { (t.loop_end - t.offset).max(t.loop_end - t.loop_start) } else { length };
    Some(once.min(MOST_BEATS)).filter(|l| l.is_finite() && *l > 0.0)
}

/// Where a clip starts playing its content and the loop it goes round.
#[derive(Clone, Copy)]
struct Timing {
    /// Where play starts (the start marker), in beats of the content.
    offset: f64,
    loop_start: f64,
    loop_end: f64,
    /// Whether it loops (and the loop has a length).
    looping: bool,
}

/// A clip's [`Timing`]: its play range is in the clip's 0x2adb, or before Bitwig 6, its content's 0x98f.
fn timing(doc: &Document, clip: ObjId) -> Timing {
    let range = if is_old_clip(doc, clip) {
        clip_content(doc, clip).and_then(|o| field_obj(doc, o, F_OLD_PLAY_RANGE))
    } else {
        field_obj(doc, clip, F_PLAY_RANGE)
    };
    let loop_range = range.and_then(|r| field_obj(doc, r, F_LOOP_RANGE));
    // A damaged file can put the play start or the loop anywhere: beyond what an arrangement can hold, it's left out.
    let num = |o: Option<ObjId>, field: u32| {
        o.and_then(|o| doc.obj(o).num(field)).filter(|v| v.is_finite() && v.abs() <= MOST_BEATS)
    };
    let offset =
        num(range.and_then(|r| field_obj(doc, r, F_PLAY_START)), F_START).or(num(loop_range, F_START)).unwrap_or(0.0);
    // A loop whose start was left out isn't moved to 0: it doesn't loop (a loop the file doesn't hold would repeat).
    let start = num(loop_range, F_START);
    let loop_start = start.unwrap_or(0.0);
    let loop_end = loop_start + num(loop_range, F_DURATION).unwrap_or(0.0);
    let dropped = start.is_none() && loop_range.is_some_and(|o| doc.obj(o).num(F_START).is_some());
    let looping = num(range, F_LOOP_ON).is_some_and(|v| v > 0.0) && !dropped && loop_end > loop_start;
    Timing { offset, loop_start, loop_end, looping }
}

/// The scenes' names, in order (Bitwig 6: root → 0x190 → 0x352f → 0x2ddb → 0x293, each name in 0x109; empty when the
/// scene has none). Empty when the project keeps them elsewhere.
fn scene_names(doc: &Document, root: ObjId) -> Vec<String> {
    [F_SCENES_HOLDER, F_SCENES_LANES, F_SCENES_LIST]
        .into_iter()
        .try_fold(root, |o, f| field_obj(doc, o, f))
        .map(|list| {
            doc.field_list(list, F_SCENES).into_iter().map(|s| doc.obj(s).str(F_SCENE_NAME).to_owned()).collect()
        })
        .unwrap_or_default()
}

/// A track's arranger lane (its timeline's clips) and its clip launcher: track → 0x2bf8 → 0x2d8e and 0x2d8f, or
/// before Bitwig 6, track → 0x15d → 0x238 and track → 0x15e.
fn track_lanes(doc: &Document, track: ObjId) -> (Option<ObjId>, Option<ObjId>) {
    match field_obj(doc, track, F_TRACK_LANES) {
        Some(lanes) => (field_obj(doc, lanes, F_ARRANGER_LANE), field_obj(doc, lanes, F_LAUNCHER_LANE)),
        None => (
            field_obj(doc, track, F_OLD_TIMELINE).and_then(|t| field_obj(doc, t, F_OLD_ARRANGER_LANE)),
            field_obj(doc, track, F_OLD_LAUNCHER),
        ),
    }
}

/// A track's clip launcher slots, one per scene (the launcher's 0x231).
fn launcher_slots(doc: &Document, track: ObjId) -> Vec<ObjId> {
    track_lanes(doc, track).1.map(|l| doc.field_list(l, F_LAUNCHER_SLOTS)).unwrap_or_default()
}

/// A clip as Bitwig wrote it before version 6: no 0x2ae9 holder, its content straight in 0x288.
fn is_old_clip(doc: &Document, clip: ObjId) -> bool {
    let c = doc.obj(clip);
    !c.has(F_CLIP_CONTENT) && c.has(F_OLD_CLIP_CONTENT)
}

/// A clip's content (its notes or audio): clip → 0x2ae9 → 0x10e9, or before Bitwig 6, clip → 0x288.
fn clip_content(doc: &Document, clip: ObjId) -> Option<ObjId> {
    if is_old_clip(doc, clip) {
        field_obj(doc, clip, F_OLD_CLIP_CONTENT)
    } else {
        field_obj(doc, clip, F_CLIP_CONTENT).and_then(|holder| field_obj(doc, holder, F_CONTENT_OBJECT))
    }
}

fn is_clip(doc: &Document, o: ObjId) -> bool {
    let c = doc.obj(o);
    matches!(c.class, CLS_CLIP | CLS_AUDIO_CLIP)
        && c.has(F_START)
        && c.has(F_DURATION)
        && (c.has(F_CLIP_CONTENT) || c.has(F_OLD_CLIP_CONTENT))
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
    let other_track = |o: ObjId| {
        let o = doc.obj(o);
        o.has(F_TRACK_LANES) || o.has(F_OLD_TIMELINE) || o.has(F_OLD_LAUNCHER) || o.has(F_GROUP_TRACKS)
    };
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
/// it from its play start (the start marker), and when looping is on, goes back to the loop range's start each time it
/// reaches its end. `play_for` is how long a looping launcher clip plays for (a scene's length); a clip that doesn't
/// loop stops at its own end.
fn clip(doc: &Document, clip: ObjId, budget: &mut NoteBudget, play_for: Option<f64>) -> Option<PreviewClip> {
    let c = doc.obj(clip);
    let (start, length) = (c.num(F_START)?, c.num(F_DURATION)?);
    // A damaged file can say anything: a clip with no length, or one that goes on for ever, isn't drawn.
    if !start.is_finite() || !length.is_finite() || length <= 0.0 {
        return None;
    }
    let muted = c.num(F_MUTED).is_some_and(|m| m > 0.0);
    let content = clip_content(doc, clip);
    let old = is_old_clip(doc, clip);
    let Timing { offset, loop_start, loop_end, looping } = timing(doc, clip);
    let length = match play_for {
        Some(play_for) if looping && play_for.is_finite() && play_for > 0.0 => play_for,
        _ => length,
    };
    let key_lanes = content.and_then(|o| field_obj(doc, o, F_NOTE_CONTENT)).map(|o| doc.field_list(o, F_KEY_LANES));
    let key_lanes = key_lanes.unwrap_or_default();
    let name =
        if old { c.str(F_OLD_CLIP_NAME) } else { content.map(|o| doc.obj(o).str(F_CONTENT_NAME)).unwrap_or_default() };

    // Where a moment of the content falls in the clip: each pass of the loop after the first starts at the loop start.
    // Worked out one at a time (as the C# app's iterator did), so a note stops as soon as the budget runs out.
    let pass_length = loop_end - loop_start;
    let placements = |at: f64| {
        let in_first = at >= offset && at - offset < length && (!looping || at < loop_end);
        let first = in_first.then_some(at - offset);
        // Passes before the clip's start (a play start after the loop's end) aren't heard: the first one heard is
        // found straight away. A loop too short to move a huge start on ends there. However short the loop, no more
        // places than the notes an arrangement holds (and one more, so the budget sees it ran out).
        let passes = (looping && at >= loop_start && at < loop_end)
            .then_some(loop_end - offset + (at - loop_start))
            .map(|p| if p < 0.0 { p.rem_euclid(pass_length) } else { p });
        let later = std::iter::successors(passes, move |&p| Some(p + pass_length).filter(|&next| next > p))
            .take(MOST_NOTES + 1)
            .take_while(move |&p| p < length);
        first.into_iter().chain(later).take(MOST_NOTES + 1)
    };

    if c.class == CLS_AUDIO_CLIP {
        let windows = audio_windows(&Timing { offset, loop_start, loop_end, looping }, length);
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
            let from = ev.num(F_SAMPLE_START).filter(|f| f.is_finite()).unwrap_or(0.0);
            // An end only counts when the event has one: a missing one doesn't make the event play backwards.
            let to = ev.num(F_SAMPLE_END).filter(|t| t.is_finite());
            let warps: Vec<(f64, f64)> = play
                .and_then(|p| field_obj(doc, p, F_WARP))
                .map(|w| doc.field_list(w, F_ITEMS))
                .unwrap_or_default()
                .into_iter()
                .map(|w| (doc.obj(w).num(F_START).unwrap_or(0.0), doc.obj(w).num(F_WARP_SECOND).unwrap_or(0.0)))
                .collect();
            // Seconds of the file per beat of the event; negative when the event plays the file backwards.
            let rate = match (warps.first(), warps.last()) {
                (Some(a), Some(b)) if warps.len() >= 2 && b.0 > a.0 => Some((b.1 - a.1) / (b.0 - a.0)),
                _ => to.filter(|&t| t != from).map(|t| (t - from) / e_length),
            }
            .filter(|r| r.is_finite());
            let settings = play.map(|p| doc.obj(p));
            let flagged = settings.and_then(|p| p.num(F_REVERSED)).is_some_and(|v| v > 0.0);
            // Where the file is at each beat of the event: `at` beats in plays second `from + at * rate`. A reversed
            // flag on forward markers plays the same stretch of file from its end.
            let (from, rate) = match rate {
                Some(r) if flagged && r > 0.0 => (from + e_length * r, Some(-r)),
                _ => (from, rate),
            };
            let stretch = settings
                .and_then(|p| p.num(F_STRETCH_MODE))
                .filter(|m| m.is_finite() && m.fract() == 0.0)
                .map(|m| Stretch::from_bitwig(m as i32));
            let semitones = play.and_then(|p| expression(doc, p, F_TRANSPOSE)).unwrap_or(0.0);
            let gain_db = play
                .and_then(|p| expression(doc, p, F_GAIN))
                .map_or(0.0, |v| if v > 0.0 { 60.0 * v.log10() } else { f64::NEG_INFINITY });
            for &(w_from, w_to, at) in &windows {
                // The part of the event this pass plays.
                let shown_from = e_start.max(w_from);
                let shown_to = (e_start + e_length).min(w_to);
                if shown_to <= shown_from {
                    continue;
                }
                if parts.len() >= MOST_AUDIO_PARTS || !budget.take() {
                    break;
                }
                let length = shown_to - shown_from;
                let backwards = rate.is_some_and(|r| r < 0.0);
                // Keep the rate positive and `from` the lowest second played, which is how a reversed part is drawn.
                let (start_in_file, rate) = match rate {
                    Some(r) => {
                        let a = from + (shown_from - e_start) * r;
                        let lowest = a.min(a + length * r);
                        // Rounding can put the lowest second a hair before the file's start; more than that means the
                        // markers don't fit the file, so the part is shown without a waveform.
                        if lowest < -1e-3 { (0.0, None) } else { (lowest.max(0.0), Some(r.abs())) }
                    }
                    None => (from, None),
                };
                parts.push(AudioPart {
                    path: file.to_owned(),
                    start: at + (shown_from - w_from),
                    length,
                    from: start_in_file,
                    seconds_per_beat: rate,
                    whole_file_beats: None,
                    reversed: backwards || flagged,
                    stretch,
                    semitones,
                    gain_db,
                });
            }
        }
        parts.sort_by(|a, b| a.start.total_cmp(&b.start));
        let name = if name.is_empty() {
            parts.first().map(|p| crate::paths::file_name(&p.path).to_owned()).unwrap_or_default()
        } else {
            name.to_owned()
        };
        let fade = |field: u32| c.num(field).filter(|f| f.is_finite() && *f > 0.0).unwrap_or(0.0).min(length);
        return Some(PreviewClip {
            name,
            start,
            length,
            muted,
            audio: parts,
            fade_in: fade(F_FADE_IN),
            fade_out: fade(F_FADE_OUT),
            ..PreviewClip::default()
        });
    }

    let mut notes = Vec::new();
    for &lane in &key_lanes {
        if budget.ran_out {
            break;
        }
        let Some(key) = doc.obj(lane).num(F_PITCH) else { continue };
        for n in doc.field_list(lane, F_ITEMS) {
            if budget.ran_out {
                break;
            }
            let n = doc.obj(n);
            if n.class != CLS_NOTE {
                continue;
            }
            let at = n.num(F_START).unwrap_or(0.0);
            let duration = n.num(F_DURATION).unwrap_or(0.25);
            let velocity = velocity::from_unit(n.num(F_VELOCITY).unwrap_or(0.8));
            for placed in placements(at) {
                if !budget.take() {
                    break;
                }
                notes.push(PreviewNote {
                    start: placed,
                    length: duration.min(length - placed),
                    key: (key as i32).clamp(0, 127) as u8,
                    velocity,
                    muted: n.num(F_MUTED).is_some_and(|m| m > 0.0),
                    step: false,
                    part: 0,
                });
            }
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    Some(PreviewClip { name: name.to_owned(), start, length, muted, notes, ..PreviewClip::default() })
}

/// The stretches of an audio clip's content that play, each as (from, to, where it starts in the clip), for a clip
/// `length` beats long: the first pass from the play start, then the loop range again and again while looping.
/// However short the loop, no more than [`MOST_PASSES`] steps.
fn audio_windows(t: &Timing, length: f64) -> Vec<(f64, f64, f64)> {
    let Timing { offset, loop_start, loop_end, looping } = *t;
    let mut windows = Vec::new();
    let first_end = if looping { loop_end.min(offset + length) } else { offset + length };
    if first_end > offset {
        windows.push((offset, first_end, 0.0));
    }
    if !looping {
        return windows;
    }
    let pass_length = loop_end - loop_start;
    // Where each pass starts in the clip, as for notes: a play start after the loop's end puts the first passes
    // before the clip's start, and only what's after it is heard. The pass the clip's start falls in is found straight
    // away: `at` is in (-pass_length, 0] then.
    let mut at = loop_end - offset;
    if at < 0.0 {
        let into = at.rem_euclid(pass_length);
        at = if into == 0.0 { 0.0 } else { into - pass_length };
    }
    for _ in 0..MOST_PASSES {
        if at >= length {
            break;
        }
        let (w_from, pos) = (loop_start + (-at).max(0.0), at.max(0.0));
        let w_to = loop_start + (length - at).min(pass_length);
        if w_to > w_from {
            windows.push((w_from, w_to, pos));
        }
        at += pass_length;
    }
    windows
}

/// The value of an expression in an audio playback's `field` at the event's start (its earliest point's 0x28f), when
/// it has one.
fn expression(doc: &Document, play: ObjId, field: u32) -> Option<f64> {
    let e = field_obj(doc, play, field)?;
    doc.field_list(e, F_ITEMS)
        .into_iter()
        .map(|p| doc.obj(p))
        .filter_map(|p| Some((p.num(F_START).unwrap_or(0.0), p.num(F_EXPRESSION_VALUE).filter(|v| v.is_finite())?)))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_huge_play_start_on_a_looping_audio_clip_is_found_at_once() {
        // Review B-1: walking pass by pass from a play start far after the loop never ended.
        let t = |offset: f64| Timing { offset, loop_start: 0.0, loop_end: 3.0, looping: true };
        let w = audio_windows(&t(1.0e7 + 1.5), 6.0);
        assert!(!w.is_empty() && w.len() <= 4, "{w:?}");
        assert!(w.iter().all(|&(from, to, at)| (0.0..=3.0).contains(&from) && to <= 3.0 && (0.0..6.0).contains(&at)));
        // Every step counts, even ones that place nothing.
        let tiny = Timing { offset: 0.0, loop_start: 0.0, loop_end: 1.0e-9, looping: true };
        assert!(audio_windows(&tiny, 1.0e6).len() <= MOST_PASSES + 1);
    }

    #[test]
    fn colours_from_floats() {
        assert_eq!(colour(Some(&Value::Color([1.0, 0.5, 0.0, 1.0]))), Some(0xFF_80_00));
        assert_eq!(colour(Some(&Value::Floats(vec![2.0, -1.0, 0.2]))), Some(0xFF_00_33));
        assert_eq!(colour(Some(&Value::Floats(vec![1.0]))), None);
        assert_eq!(colour(None), None);
    }
}
