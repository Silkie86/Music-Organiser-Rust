//! The automation clips in a Bitwig project: clips on a setting's automation lane, each with its own points (as
//! Bitwig 6.1.6 saves them, worked out from Solomon's MO Live Test saves, 2026-11-01 and 02). For the curves library:
//! each clip's shape can become a curve file ([`super::curve`]).
//!
//! A clip (class 0xd90) sits in its lane's timeline (lane 0x23d, field 0x2d8c, list 0x21f): 0x2af where it starts and
//! 0x26 its length in beats, 0x2d24 its name (a dropped curve's), and its points in 0x2ae9 . 0x10e9 . 0x48e . 0x21f,
//! each with 0x2af its beat from the clip's start, 0x28f its value and 0xba6 how the line to the next one bends. The
//! lane's 0x2a3 says what it moves: 0x133b the device ("Transport", "Mixer", a plugin) and 0x133a the setting.

use super::format::{Document, ObjId};

const CLIP: u32 = 0xd90;
const LANE: u32 = 0x23d;

/// A point of an automation clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipPoint {
    /// Beats from the clip's start.
    pub beat: f64,
    /// The value as the lane saves it: 0 to 1 for a plugin's setting; real BPM on the tempo lane.
    pub value: f64,
    /// How the line to the next point bends, -1 to 1 (0 straight).
    pub bend: f64,
}

/// An automation clip in a project.
#[derive(Clone, Debug, PartialEq)]
pub struct AutomationClip {
    /// The clip's own name (a dropped curve's), or "".
    pub name: String,
    /// What the lane moves: the device ("Mixer", "Transport", a plugin's name) and the setting ("Volume", "Cutoff").
    pub device: String,
    pub setting: String,
    /// Where it starts in the song and how long it is, in beats.
    pub start: f64,
    pub length: f64,
    pub muted: bool,
    pub points: Vec<ClipPoint>,
}

/// Every automation clip in the project, in the order the file has them. A clip with no points is left out.
pub fn clips(doc: &Document) -> Vec<AutomationClip> {
    (0..doc.objects.len() as u32)
        .map(ObjId)
        .filter(|&o| doc.obj(o).class == CLIP)
        .filter_map(|o| clip(doc, o))
        .collect()
}

fn clip(doc: &Document, o: ObjId) -> Option<AutomationClip> {
    let c = doc.obj(o);
    let content = doc.field_obj(o, 0x2ae9).and_then(|h| doc.field_obj(h, 0x10e9))?;
    let list = doc.field_obj(content, 0x48e)?;
    let mut points: Vec<ClipPoint> = doc
        .field_list(list, 0x21f)
        .into_iter()
        .map(|p| doc.obj(p))
        .filter_map(|p| {
            Some(ClipPoint { beat: p.num(0x2af)?, value: p.num(0x28f)?, bend: p.num(0xba6).unwrap_or(0.0) })
        })
        .filter(|p| p.beat.is_finite() && p.value.is_finite())
        .collect();
    if points.is_empty() {
        return None;
    }
    points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    let (device, setting) = lane(doc, o)
        .and_then(|l| doc.field_obj(l, 0x2a3))
        .map(|t| (doc.obj(t).str(0x133b).to_owned(), doc.obj(t).str(0x133a).to_owned()))
        .unwrap_or_default();
    Some(AutomationClip {
        name: c.str(0x2d24).to_owned(),
        device,
        setting,
        start: c.num(0x2af).unwrap_or(0.0),
        length: c.num(0x26).unwrap_or(0.0),
        muted: c.num(0x10f8).is_some_and(|m| m != 0.0),
        points,
    })
}

/// The lane a clip was read inside, going up through the objects that hold it.
fn lane(doc: &Document, mut o: ObjId) -> Option<ObjId> {
    for _ in 0..8 {
        o = doc.obj(o).parent?.0;
        if doc.obj(o).class == LANE {
            return Some(o);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::format::{Object, Slot, Text, Value};
    use super::*;

    /// A lane moving a plugin's Cutoff, holding one clip with three points, built as Bitwig saves it.
    #[test]
    fn a_lanes_clip_gives_its_place_points_and_what_it_moves() {
        let mut doc = decode_empty();
        let point = |doc: &mut Document, beat: f64, value: f64, bend: f64| {
            let mut p = Object::new(0x108);
            p.set(0x2af, Value::F64(beat));
            p.set(0x28f, Value::F64(value));
            p.set(0xba6, Value::F64(bend));
            doc.add(p)
        };
        let ps = [point(&mut doc, 2.0, 1.0, 0.0), point(&mut doc, 0.0, 0.0, 0.82), point(&mut doc, 4.0, 0.0, 0.0)];
        let mut list = Object::new(0x6e);
        list.set(0x21f, Value::List(ps.iter().map(|&p| Slot::Obj(p)).collect()));
        let list = doc.add(list);
        let mut content = Object::new(0x3c);
        content.set(0x48e, Value::Obj(Slot::Obj(list)));
        let content = doc.add(content);
        let mut holder = Object::new(0xebc);
        holder.set(0x10e9, Value::Obj(Slot::Obj(content)));
        let holder = doc.add(holder);
        let mut clip = Object::new(CLIP);
        clip.set(0x2af, Value::F64(28.0));
        clip.set(0x26, Value::F64(4.0));
        clip.set(0x2d24, Value::Str(Text::new("Basic Sequence #1")));
        clip.set(0x2ae9, Value::Obj(Slot::Obj(holder)));
        let clip = doc.add(clip);
        let mut target = Object::new(0xf1);
        target.set(0x133b, Value::Str(Text::new("Acid V")));
        target.set(0x133a, Value::Str(Text::new("Cutoff")));
        let target = doc.add(target);
        let mut timeline = Object::new(0xd91);
        timeline.set(0x21f, Value::List(vec![Slot::Obj(clip)]));
        let timeline = doc.add(timeline);
        let mut lane = Object::new(LANE);
        lane.set(0x2a3, Value::Obj(Slot::Obj(target)));
        lane.set(0x2d8c, Value::Obj(Slot::Obj(timeline)));
        let lane = doc.add(lane);
        doc.obj_mut(clip).parent = Some((timeline, 0x21f));
        doc.obj_mut(timeline).parent = Some((lane, 0x2d8c));

        let found = clips(&doc);
        assert_eq!(found.len(), 1);
        let c = &found[0];
        assert_eq!((c.name.as_str(), c.device.as_str(), c.setting.as_str()), ("Basic Sequence #1", "Acid V", "Cutoff"));
        assert_eq!((c.start, c.length, c.muted), (28.0, 4.0, false));
        let beats: Vec<(f64, f64, f64)> = c.points.iter().map(|p| (p.beat, p.value, p.bend)).collect();
        assert_eq!(beats, [(0.0, 0.0, 0.82), (2.0, 1.0, 0.0), (4.0, 0.0, 0.0)]);
    }

    fn decode_empty() -> Document {
        let mut raw = b"BtWg".to_vec();
        raw.extend_from_slice(format!("000200010000{:08x}{:016x}00", 42, 0).as_bytes());
        super::super::format::decode(raw).unwrap()
    }
}
