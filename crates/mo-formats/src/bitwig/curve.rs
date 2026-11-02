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

use super::format::{Document, Value, decode};

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

/// How Bitwig files a curve: a one-off run of moves, or a shape that repeats (as an LFO's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Sequence,
    Periodic,
}

impl Category {
    fn name(self) -> &'static str {
        match self {
            Category::Sequence => "Sequence",
            Category::Periodic => "Periodic",
        }
    }
}

/// A curve file.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub name: String,
    pub creator: String,
    pub comment: String,
    /// "Sequence" or "Periodic" as saved (another word is kept as it is).
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
        beat_length: meta_num(&doc, "beat_length"),
        points,
    })
}

fn meta_num(doc: &Document, key: &str) -> Option<f64> {
    doc.meta.iter().find(|(k, _)| k.as_str() == key).and_then(|(_, v)| match v {
        Value::F64(x) => Some(*x),
        _ => None,
    })
}

/// A curve file for `curve`, as Bitwig saves one. `id` is the revision id Bitwig keeps in the metadata (40 hex
/// characters; any that tell curves apart will do). The x of each point is kept within 0 to 1 and in order, y within
/// 0 to 1 and the bend within -1 to 1, so Bitwig never gets a point it couldn't have saved. Written from Gararge 1's
/// numbers, the body comes out byte for byte as Bitwig saved it (checked outside the repo, 2026-11-02).
pub fn write(curve: &Curve, category: Category, id: &str) -> Vec<u8> {
    let mut meta = Vec::new();
    u32(&mut meta, 4);
    text(&mut meta, "meta");
    let mut entry = |key: &str, value: &dyn Fn(&mut Vec<u8>)| {
        u32(&mut meta, 1);
        text(&mut meta, key);
        value(&mut meta);
    };
    // In the order Bitwig writes them (by name).
    entry("application_version_name", &|m| string(m, "5.0"));
    if let Some(beats) = curve.beat_length {
        entry("beat_length", &|m| {
            m.push(0x07);
            m.extend_from_slice(&beats.to_be_bytes());
        });
    }
    entry("branch", &|m| string(m, "releases"));
    entry("comment", &|m| string(m, &curve.comment));
    entry("creator", &|m| string(m, &curve.creator));
    entry("curve_category", &|m| string(m, category.name()));
    entry("curve_kind", &|m| m.extend_from_slice(&[0x01, 0]));
    entry("revision_id", &|m| string(m, id));
    entry("revision_no", &|m| {
        m.push(0x03);
        m.extend_from_slice(&1i32.to_be_bytes());
    });
    entry("tags", &|m| string(m, &curve.tags));
    entry("type", &|m| string(m, TYPE));
    u32(&mut meta, 0);
    // Bitwig leaves room so it can save the metadata again without moving the body.
    meta.extend(std::iter::repeat_n(b' ', 5000));
    meta.push(b'\n');

    let mut body = Vec::new();
    u32(&mut body, CURVE);
    field(&mut body, 0x362d, &[0x01, 0]);
    field(&mut body, 0x3646, &[0x05, 1]);
    field(&mut body, 0x3601, &[0x01, 16]);
    // Unknown: 1 in Solomon's Sequence curve, 4 in his two Periodic ones; kept as they have it.
    field(&mut body, 0x3602, &[0x01, if category == Category::Periodic { 4 } else { 1 }]);
    field(&mut body, 0x3635, &[0x01, 0]);
    u32(&mut body, POINTS);
    body.push(0x12);
    let mut last_x = 0.0f64;
    for p in &curve.points {
        let x = clean(p.x, 0.0, 1.0).max(last_x);
        last_x = x;
        u32(&mut body, POINT);
        for (id, v) in [(Y, clean(p.y, 0.0, 1.0)), (X, x), (BEND, clean(p.bend, -1.0, 1.0))] {
            u32(&mut body, id);
            body.push(0x07);
            body.extend_from_slice(&v.to_be_bytes());
        }
        u32(&mut body, 0);
    }
    u32(&mut body, 3);
    for id in [0x3788, 0x3665, 0x3666] {
        field(&mut body, id, &[0x01, 0xff]);
    }
    for (id, s) in [
        (NAME, &curve.name),
        (CREATOR, &curve.creator),
        (COMMENT, &curve.comment),
        (CATEGORY, &category.name().to_owned()),
        (TAGS, &curve.tags),
    ] {
        u32(&mut body, id);
        string(&mut body, s);
    }
    u32(&mut body, 0);

    let body_offset = super::format::HEADER + meta.len();
    let mut file = Vec::with_capacity(body_offset + body.len());
    file.extend_from_slice(format!("BtWg000300020000{body_offset:08x}{:016x}00", 0).as_bytes());
    file.extend_from_slice(&meta);
    file.extend_from_slice(&body);
    file
}

/// A number Bitwig can hold: within its range, and 0 when it isn't a number at all.
fn clean(v: f64, low: f64, high: f64) -> f64 {
    if v.is_finite() { v.clamp(low, high) } else { 0.0 }
}

fn u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn field(out: &mut Vec<u8>, id: u32, value: &[u8]) {
    u32(out, id);
    out.extend_from_slice(value);
}

/// A text without its type byte: its length, then its bytes; text that isn't plain ASCII goes as UTF-16 with the top
/// bit of the length set, as Bitwig writes it.
fn text(out: &mut Vec<u8>, s: &str) {
    if s.is_ascii() {
        u32(out, s.len() as u32);
        out.extend_from_slice(s.as_bytes());
    } else {
        let units: Vec<u16> = s.encode_utf16().collect();
        u32(out, 0x8000_0000 | units.len() as u32);
        out.extend(units.iter().flat_map(|u| u.to_be_bytes()));
    }
}

fn string(out: &mut Vec<u8>, s: &str) {
    out.push(0x08);
    text(out, s);
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
        let raw = write(&c, Category::Sequence, "455263ea8403a86e70124694ea938320dd24d1d2");
        assert_eq!(read(raw.clone()), Some(c.clone()));
        // The body starts where the header says, after the padding, as in Bitwig's own curves.
        let doc = decode(raw.clone()).unwrap();
        assert_eq!(raw.get(doc.body_offset - 1), Some(&b'\n'));
        assert!(doc.meta.iter().any(|(k, v)| k.as_str() == "type" && matches!(v, Value::Str(t) if t.as_str() == TYPE)));
        // And it comes back byte for byte through the project writer.
        assert_eq!(super::super::encode(&doc, None).unwrap(), raw);
    }

    #[test]
    fn names_that_arent_plain_ascii_and_wild_numbers_are_kept_safe() {
        let mut c =
            curve(vec![CurvePoint { x: 0.5, y: 2.0, bend: f64::NAN }, CurvePoint { x: 0.25, y: -1.0, bend: -3.0 }]);
        c.name = "Café – Encoder1".into();
        c.beat_length = None;
        let back = read(write(&c, Category::Periodic, "0")).unwrap();
        assert_eq!(back.name, "Café – Encoder1");
        assert_eq!(back.category, "Periodic");
        assert_eq!(back.beat_length, None);
        assert_eq!(back.points, [CurvePoint { x: 0.5, y: 1.0, bend: 0.0 }, CurvePoint { x: 0.5, y: 0.0, bend: -1.0 }]);
    }

    #[test]
    fn something_else_isnt_a_curve() {
        assert_eq!(read(b"not a curve".to_vec()), None);
    }
}
