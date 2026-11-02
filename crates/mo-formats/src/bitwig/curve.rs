//! Bitwig Studio's curve files (`.bwcurve`): a shape saved in the library that can be dropped onto an automation lane
//! (it becomes an automation clip there, named after the curve) or used by a modulator.
//!
//! Worked out from Solomon's own curves (Filter Rhythm 001 2, Gararge 1 and London City, saved in Bitwig 5.0 to 6.0)
//! and a drop test (2026-11-02). The file is laid out as a project is (format 0003 0002, no zip): metadata, then
//! padding, then one object, class 0x127f, holding the points (class 0x127e: y, x and bend). Curves from Bitwig's own
//! packs say 0003 0004 and their body is locked, so they can't be read.
//!
//! - x runs from 0 to 1 across the curve, in order; two points on one x make a jump.
//! - y is the value, 0 to 1.
//! - bend (-1 to 1, 0 straight) bends the line from this point to the next. Dropped on a lane it becomes the point's
//!   curve (0xba6) as it is: Basic Sequence 1's 0.82, -1 and 1 came across unchanged.

use std::sync::Arc;

use super::encode::{encode, fresh_id};
use super::format::{Document, HEADER, ObjId, Object, Slot, Text, Value, decode};

/// The metadata's type for a curve.
pub const TYPE: &str = "application/bitwig-curve";
/// The file ending.
pub const EXTENSION: &str = "bwcurve";

const CURVE: u32 = 0x127f;
const POINT: u32 = 0x127e;
const POINTS: u32 = 0x3603;
const Y: u32 = 0x35fd;
const X: u32 = 0x35fe;
const BEND: u32 = 0x35ff;
const NAME: u32 = 0x360b;
const CREATOR: u32 = 0x3621;
const COMMENT: u32 = 0x3623;
const CATEGORY: u32 = 0x3622;
const TAGS: u32 = 0x36a6;

/// One point of a curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurvePoint {
    /// Across the curve, 0 to 1.
    pub x: f64,
    /// The value, 0 to 1.
    pub y: f64,
    /// How the line to the next point bends, -1 to 1 (0 is straight).
    pub bend: f64,
}

/// A curve file.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub name: String,
    pub creator: String,
    pub comment: String,
    /// How Bitwig files it: "Sequence" (a one-off run of moves) or "Periodic" (a shape that repeats, as an LFO's);
    /// another word is kept as it is.
    pub category: String,
    /// Space-separated, as Bitwig saves them.
    pub tags: String,
    /// How long it was, in beats, when Bitwig keeps that (only some curves do).
    pub beat_length: Option<f64>,
    pub points: Vec<CurvePoint>,
}

/// A curve file's contents; None when it isn't a curve or its body can't be read (a pack's locked curve).
pub fn read(raw: Vec<u8>) -> Option<Curve> {
    let doc = decode(raw).ok()?;
    if doc.body_error.is_some() {
        return None;
    }
    let root = doc.root?;
    let curve = doc.obj(root);
    if curve.class != CURVE {
        return None;
    }
    let points = doc
        .field_list(root, POINTS)
        .into_iter()
        .map(|p| doc.obj(p))
        .filter(|p| p.class == POINT)
        .map(|p| CurvePoint {
            x: p.num(X).unwrap_or(0.0),
            y: p.num(Y).unwrap_or(0.0),
            bend: p.num(BEND).unwrap_or(0.0),
        })
        .collect();
    Some(Curve {
        name: curve.str(NAME).to_owned(),
        creator: curve.str(CREATOR).to_owned(),
        comment: curve.str(COMMENT).to_owned(),
        category: curve.str(CATEGORY).to_owned(),
        tags: curve.str(TAGS).to_owned(),
        beat_length: doc.meta("beat_length").and_then(number),
        points,
    })
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::F64(x) => Some(*x),
        Value::F32(x) => Some(f64::from(*x)),
        Value::I32(x) => Some(f64::from(*x)),
        Value::I16(x) => Some(f64::from(*x)),
        _ => None,
    }
}

/// A curve file for `curve`, as Bitwig saves one; None when it has no points. Points are put in order across the
/// curve (two on one x keep their order, making a jump), x and y are kept within 0 to 1 and the bend within -1 to 1,
/// and a single point becomes a flat line, so Bitwig never gets a curve it couldn't have saved. Each file gets its own
/// revision id. Written from Gararge 1's numbers, the body comes out byte for byte as Bitwig saved it (checked outside
/// the repo, 2026-11-02).
pub fn write(curve: &Curve) -> Option<Vec<u8>> {
    let mut points: Vec<(f64, f64, f64)> =
        curve.points.iter().map(|p| (clean(p.x, 0.0, 1.0), clean(p.y, 0.0, 1.0), clean(p.bend, -1.0, 1.0))).collect();
    points.sort_by(|a, b| a.0.total_cmp(&b.0));
    match points.as_slice() {
        [] => return None,
        [(_, y, _)] => points = vec![(0.0, *y, 0.0), (1.0, *y, 0.0)],
        _ => {}
    }

    let text = |s: &str| Value::Str(Text::new(s));
    let mut meta = vec![("application_version_name", text("5.0"))];
    if let Some(beats) = curve.beat_length.filter(|b| b.is_finite() && *b > 0.0) {
        meta.push(("beat_length", Value::F64(beats)));
    }
    // In the order Bitwig writes them (by name).
    meta.extend([
        ("branch", text("releases")),
        ("comment", text(&curve.comment)),
        ("creator", text(&curve.creator)),
        ("curve_category", text(&curve.category)),
        ("curve_kind", Value::U8(0)),
        ("revision_id", text(&revision_id())),
        ("revision_no", Value::I32(1)),
        ("tags", text(&curve.tags)),
        ("type", text(TYPE)),
    ]);

    let mut objects = Vec::with_capacity(points.len() + 1);
    let mut root = Object::new(CURVE);
    root.set(0x362d, Value::U8(0));
    root.set(0x3646, Value::Bool(1));
    root.set(0x3601, Value::U8(16));
    // Unknown: 1 in Solomon's Sequence curve, 4 in his two Periodic ones; kept as they have it.
    root.set(0x3602, Value::U8(if curve.category == "Periodic" { 4 } else { 1 }));
    root.set(0x3635, Value::U8(0));
    root.set(POINTS, Value::List((1..=points.len() as u32).map(|n| Slot::Obj(ObjId(n))).collect()));
    for id in [0x3788, 0x3665, 0x3666] {
        root.set(id, Value::U8(0xff));
    }
    for (id, s) in [
        (NAME, &curve.name),
        (CREATOR, &curve.creator),
        (COMMENT, &curve.comment),
        (CATEGORY, &curve.category),
        (TAGS, &curve.tags),
    ] {
        root.set(id, text(s));
    }
    objects.push(root);
    for (x, y, bend) in points {
        let mut point = Object::new(POINT);
        point.set(Y, Value::F64(y));
        point.set(X, Value::F64(x));
        point.set(BEND, Value::F64(bend));
        objects.push(point);
    }

    // The metadata's end marker, then the room Bitwig leaves so it can save the metadata again without moving the
    // body; encode copies these from the "file as it was" (from meta_end to the body). meta_end sits one byte past the
    // header, as encode writes metadata only for a document that had some.
    let meta_end = HEADER + 1;
    let mut raw = vec![0u8; meta_end + 4];
    raw.extend(std::iter::repeat_n(b' ', 5000));
    raw.push(b'\n');
    let mut header = [b'0'; 38];
    header[..12].copy_from_slice(b"000300020000");
    let doc = Document {
        body_offset: raw.len(),
        zip_offset: raw.len(),
        raw: Arc::new(raw),
        header,
        meta: meta.into_iter().map(|(k, v)| (Text::new(k), v)).collect(),
        meta_end,
        root: Some(ObjId(0)),
        objects,
        has_zip: false,
        body_error: None,
    };
    encode(&doc, None).ok()
}

/// 40 hex characters, as Bitwig's revision ids are, new each time.
fn revision_id() -> String {
    let (a, b) = (fresh_id(), fresh_id());
    a.iter().chain(&b[..4]).map(|x| format!("{x:02x}")).collect()
}

/// A number Bitwig can hold: within its range, and 0 when it isn't a number at all.
fn clean(v: f64, low: f64, high: f64) -> f64 {
    if v.is_finite() { v.clamp(low, high) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(points: Vec<CurvePoint>) -> Curve {
        Curve {
            name: "sly_17 - Pattern 3 - Encoder1".into(),
            creator: "Music Organiser".into(),
            comment: String::new(),
            category: "Sequence".into(),
            tags: "fl".into(),
            beat_length: Some(16.0),
            points,
        }
    }

    #[test]
    fn a_written_curve_reads_back_as_it_was() {
        let points = vec![
            CurvePoint { x: 0.0, y: 0.0, bend: 1.0 },
            CurvePoint { x: 0.0625, y: 1.0, bend: 0.0 },
            CurvePoint { x: 0.1875, y: 0.0, bend: 0.0 },
            CurvePoint { x: 0.1875, y: 1.0, bend: -0.48 },
            CurvePoint { x: 1.0, y: 0.0, bend: 0.0 },
        ];
        let c = curve(points);
        let raw = write(&c).unwrap();
        assert_eq!(read(raw.clone()), Some(c.clone()));
        // The body starts where the header says, after the padding, as in Bitwig's own curves.
        let doc = decode(raw.clone()).unwrap();
        assert_eq!(raw.get(doc.body_offset - 1), Some(&b'\n'));
        assert!(doc.meta.iter().any(|(k, v)| k.as_str() == "type" && matches!(v, Value::Str(t) if t.as_str() == TYPE)));
        // And it comes back byte for byte through the project writer.
        assert_eq!(encode(&doc, None).unwrap(), raw);
        assert_eq!(doc.meta_str("revision_id").len(), 40);
    }

    #[test]
    fn names_that_arent_plain_ascii_and_wild_numbers_are_kept_safe() {
        let mut c =
            curve(vec![CurvePoint { x: 0.5, y: 2.0, bend: f64::NAN }, CurvePoint { x: 0.25, y: -1.0, bend: -3.0 }]);
        c.name = "Café – Encoder1".into();
        c.beat_length = None;
        c.category = "Periodic".into();
        let back = read(write(&c).unwrap()).unwrap();
        assert_eq!(back.name, "Café – Encoder1");
        assert_eq!(back.category, "Periodic");
        assert_eq!(back.beat_length, None);
        assert_eq!(back.points, [CurvePoint { x: 0.25, y: 0.0, bend: -1.0 }, CurvePoint { x: 0.5, y: 1.0, bend: 0.0 }]);
    }

    #[test]
    fn something_else_isnt_a_curve() {
        assert_eq!(read(b"not a curve".to_vec()), None);
    }
}
