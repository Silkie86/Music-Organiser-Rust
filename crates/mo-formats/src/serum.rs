//! Xfer's LFO shape files: Serum 2's `.XferShape`, Serum 1's (and LFOTool's) `.shp`, and LFOTool's presets (`.fxp`,
//! twelve graphs each). A shape is the line an LFO draws, repeated;
//! each is read as points across (0 to 1), value (0 to 1, 1 at the top) and how the line to the next point bends.
//!
//! Worked out (2026-11-13) from Solomon's own shapes: the 19 rhythmic ones he sent, and his `Serum 2 Presets\LFO
//! Shapes` folder, where most of Serum's basic shapes are there in both formats ("saw up.shp" and "Saw Up.XferShape").
//! Those pairs settle the two things the bytes alone don't: which way up a value is, and which way a bend goes.
//!
//! - **`.XferShape`** (Serum 2 and Serum 2 FX, file version 4.0 and 5.0): `XferJson` and a 0 byte, the length of a JSON
//!   header (8 bytes, low byte first), the header (`fileType` "XferShape", `product`, `productVersion`, `version`),
//!   then the body's length when unpacked (4 bytes), a 4-byte 2, and the body packed with zstd. The body is CBOR, a map
//!   whose `LFO` map holds `xVals`, `yVals` and `curveVals` (one number a point) and `numPoints`. The lists hold one
//!   point more than `numPoints`: that last one is always 1, 1, 0.5, whatever the shape, and isn't part of it. Some
//!   files (Basic\Sine) also say `loopbackPointNum`, which isn't needed to draw the shape.
//! - **`.shp`** (Serum 1): 2,056 bytes. 64 numbers (8 bytes each) for the bends, 64 for across, in Serum 1's pixels
//!   (0 to 388), 64 for the value, in pixels (0 to 240), and 64 unused; then the number of points in one byte. The 7
//!   bytes after it differ from file to file whatever the shape (leftovers in Serum 1's memory) and are passed over.
//!
//! - **LFOTool's `.fxp`**: a VST preset (`CcnK`, `FPCh`, plugin id `XFRa`) whose 18,816-byte chunk holds twelve graphs
//!   laid out list by list: the bends of all twelve (64 numbers each), then their across, then their values, in the same
//!   pixels as a `.shp`; then 384 bytes of settings, with the twelve point counts 352 bytes in (one byte each). A graph
//!   nobody drew is LFOTool's own start (two points: top at the start, a value half way, straight) and is passed over.
//!
//! In all of them, a value is kept top down: 0 is the top of Serum's box, so "Saw Up" goes from 1 to 0. A bend (0 to 1, 0.5
//! straight) belongs to the line from its point to the next: below 0.5 the line moves early and then levels out
//! ("Saw Down Curved" drops fast), above 0.5 it stays and then moves, whichever way the line goes. The shape repeats,
//! so its last point joins the first again at the end ("Triangle" is two points: bottom at the start, top half way).

use std::io::Read;

/// Goes up by one whenever what's read from the same shape files changes (rule 7), so the curves library reads them
/// again.
pub const VERSION: u32 = 1;

/// What a `.XferShape` file starts with.
const MAGIC: &[u8] = b"XferJson\0";
/// The biggest header or unpacked body read: a shape's are a few hundred bytes, so anything near this is no shape.
const MOST_BYTES: usize = 1 << 20;
/// The biggest zstd window a body may ask for: a shape's body is a few hundred bytes, so its window is too; this
/// keeps a damaged file from making the decoder set aside its usual 100 MB.
const MOST_WINDOW: u64 = 8 << 20;
/// The most points a shape is read with (Serum 1 holds 64; Serum 2's own limit isn't known, so this is generous).
const MOST_POINTS: usize = 4096;
/// A `.shp` file's size, and the size of its box in pixels.
const SHP_BYTES: usize = 2056;
const SHP_WIDTH: f64 = 388.0;
const SHP_HEIGHT: f64 = 240.0;

/// One point of a shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapePoint {
    /// Across the shape, 0 to 1, in order; two points on one x make a jump.
    pub x: f64,
    /// The value, 0 (bottom) to 1 (top).
    pub value: f64,
    /// How the line to the next point bends, -1 to 1: 0 straight, below 0 it moves early, above 0 late.
    pub bend: f64,
}

/// An LFO shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// Which plugin saved it: "Serum 2", "Serum 2 FX", "Serum" or "LFOTool" (a `.shp` file is Serum 1's format, which
    /// LFOTool shares: it's called Serum's).
    pub product: String,
    /// Which of a preset's graphs it is ("Graph 3"); "" for a shape file, which holds one.
    pub part: String,
    /// Its points, the last one at x = 1.
    pub points: Vec<ShapePoint>,
}

/// The shapes in a file, by its name's ending (`.XferShape`, `.shp` or LFOTool's `.fxp`, big or small letters): none
/// for another file or one that isn't a shape.
pub fn read(path: &str, raw: &[u8]) -> Vec<Shape> {
    let lower = path.to_lowercase();
    if lower.ends_with(".xfershape") {
        read_xfershape(raw).into_iter().collect()
    } else if lower.ends_with(".shp") {
        read_shp(raw).into_iter().collect()
    } else if lower.ends_with(".fxp") {
        read_lfotool(raw)
    } else {
        Vec::new()
    }
}

/// Whether a file's name says it may hold shapes ([`read`]).
pub fn is_shape_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".xfershape") || lower.ends_with(".shp") || lower.ends_with(".fxp")
}

/// A Serum 2 `.XferShape` file's shape.
pub fn read_xfershape(raw: &[u8]) -> Option<Shape> {
    let rest = raw.strip_prefix(MAGIC)?;
    let (len, rest) = split_le(rest, 8)?;
    let len = usize::try_from(len).ok().filter(|l| *l <= MOST_BYTES)?;
    let (header, rest) = (rest.get(..len)?, rest.get(len..)?);
    let header: serde_json::Value = serde_json::from_slice(header).ok()?;
    if header.get("fileType")?.as_str()? != "XferShape" {
        return None;
    }
    let product = match header.get("product").and_then(|p| p.as_str()) {
        Some("Serum2FX") => "Serum 2 FX",
        _ => "Serum 2",
    };
    let (size, rest) = split_le(rest, 4)?;
    let (_, packed) = split_le(rest, 4)?;
    let size = usize::try_from(size).ok().filter(|s| *s <= MOST_BYTES)?;
    let body = unzstd(packed, size)?;
    let body = Cbor::parse(&body)?;
    let lfo = body.get("LFO")?;
    let n = usize::try_from(lfo.get("numPoints")?.int()?).ok().filter(|n| *n <= MOST_POINTS)?;
    let list = |key: &str| -> Option<Vec<f64>> { lfo.get(key)?.list()?.iter().map(Cbor::num).collect() };
    let (xs, ys, curves) = (list("xVals")?, list("yVals")?, list("curveVals")?);
    if xs.len() < n || ys.len() < n || curves.len() < n {
        return None;
    }
    let points = xs.into_iter().zip(ys).zip(curves).take(n).map(|((x, y), c)| (x, y, c));
    Some(Shape { product: product.into(), part: String::new(), points: closed(points)? })
}

/// A Serum 1 `.shp` file's shape.
pub fn read_shp(raw: &[u8]) -> Option<Shape> {
    if raw.len() != SHP_BYTES {
        return None;
    }
    let n = usize::from(*raw.get(2048)?);
    if n == 0 || n > 64 {
        return None;
    }
    let points = pixel_points(raw, n, 1, 0)?;
    Some(Shape { product: "Serum".into(), part: String::new(), points: closed(points.into_iter())? })
}

/// LFOTool's preset: its size, where its graphs start and where their point counts are.
const LFOTOOL_BYTES: usize = 18876;
const LFOTOOL_GRAPHS: usize = 60;
const LFOTOOL_COUNTS: usize = 18844;

/// The graphs someone drew in an LFOTool preset (`.fxp`), each a shape ("Graph 1" to "Graph 12").
pub fn read_lfotool(raw: &[u8]) -> Vec<Shape> {
    let preset = raw.len() == LFOTOOL_BYTES
        && raw.get(..4) == Some(b"CcnK")
        && raw.get(8..12) == Some(b"FPCh")
        && raw.get(16..20) == Some(b"XFRa")
        && raw.get(56..60) == Some(&18816u32.to_be_bytes()[..]);
    // Always there in a file of the right size; `get` all the same, as every reader here (no slicing past the end).
    let (true, Some(graphs), Some(counts)) =
        (preset, raw.get(LFOTOOL_GRAPHS..), raw.get(LFOTOOL_COUNTS..LFOTOOL_COUNTS + 12))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (g, &n) in counts.iter().enumerate() {
        let n = usize::from(n);
        if n == 0 || n > 64 {
            continue;
        }
        let Some(points) = pixel_points(graphs, n, 12, g) else { continue };
        // LFOTool's own start, in a graph nobody drew: top, then the middle or the bottom half way, straight (the
        // presets show both, by the version that saved them). A shape drawn just so is passed over with them.
        let untouched = n == 2
            && points.iter().all(|p| (p.2 - 0.5).abs() < 1e-3)
            && points.first().is_some_and(|p| p.0 == 0.0 && p.1 == 0.0)
            && points.get(1).is_some_and(|p| p.0 == 0.5 && (p.1 == 0.5 || p.1 == 1.0));
        if untouched {
            continue;
        }
        if let Some(points) = closed(points.into_iter()) {
            out.push(Shape { product: "LFOTool".into(), part: format!("Graph {}", g + 1), points });
        }
    }
    out
}

/// `n` points of shape `which`, from lists of 64 numbers (8 bytes, low byte first) laid out as Serum 1 lays them: the
/// bends of all `of` shapes (1 in a `.shp`, 12 in an LFOTool preset), then their across in pixels, then their values in
/// pixels. Gives across and value from 0 to 1 (value still top down) and the bend as saved.
fn pixel_points(raw: &[u8], n: usize, of: usize, which: usize) -> Option<Vec<(f64, f64, f64)>> {
    let num = |list: usize, i: usize| -> Option<f64> {
        let at = ((list * of + which) * 64 + i) * 8;
        Some(f64::from_le_bytes(raw.get(at..at + 8)?.try_into().ok()?))
    };
    (0..n).map(|i| Some((num(1, i)? / SHP_WIDTH, num(2, i)? / SHP_HEIGHT, num(0, i)?))).collect()
}

/// Serum's points (across 0 to 1, value top down, bend 0 to 1 with 0.5 straight) as a shape's, with the line back to
/// the first value at the end when the last point isn't there already. None when a number isn't one, or no point is
/// left.
fn closed(points: impl Iterator<Item = (f64, f64, f64)>) -> Option<Vec<ShapePoint>> {
    let mut out: Vec<ShapePoint> = Vec::new();
    for (x, y, c) in points {
        if !(x.is_finite() && y.is_finite() && c.is_finite()) {
            return None;
        }
        // Serum keeps its points in order; a point a hair before the last (Serum 2 saves -0 for 0) is put on it, and
        // adding 0 turns -0 into 0, which sorts with it.
        let before = out.last().map_or(0.0, |p| p.x);
        out.push(ShapePoint {
            x: x.clamp(before, 1.0) + 0.0,
            value: 1.0 - y.clamp(0.0, 1.0),
            bend: ((c - 0.5) * 2.0).clamp(-1.0, 1.0),
        });
    }
    let (first, last) = (*out.first()?, *out.last()?);
    if last.x < 1.0 {
        out.push(ShapePoint { x: 1.0, value: first.value, bend: 0.0 });
    }
    Some(out)
}

/// The number in the first `bytes` bytes (low byte first), and what's after them.
fn split_le(raw: &[u8], bytes: usize) -> Option<(u64, &[u8])> {
    let (head, rest) = (raw.get(..bytes)?, raw.get(bytes..)?);
    let n = head.iter().rev().fold(0u64, |n, b| (n << 8) | u64::from(*b));
    Some((n, rest))
}

/// A zstd frame unpacked, when it's `size` bytes as the file says.
fn unzstd(packed: &[u8], size: usize) -> Option<Vec<u8>> {
    let mut packed = packed;
    let decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(&mut packed, MOST_WINDOW).ok()?;
    let mut out = Vec::with_capacity(size);
    decoder.take(size as u64 + 1).read_to_end(&mut out).ok()?;
    (out.len() == size).then_some(out)
}

/// What a CBOR body holds, as far as a shape needs: only what Serum writes is read (no tags, no lists of unknown
/// length), and a body that says more than it holds, or nests very deep, isn't read at all.
#[derive(Clone, Debug, PartialEq)]
enum Cbor {
    Int(i128),
    Num(f64),
    Text(String),
    Bytes,
    List(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
    Other,
}

/// How deep lists and maps may nest.
const MOST_DEPTH: usize = 16;

impl Cbor {
    /// The whole of `raw` as one value; None when it isn't, or bytes are left over.
    fn parse(raw: &[u8]) -> Option<Cbor> {
        let mut at = raw;
        let v = Cbor::value(&mut at, 0)?;
        at.is_empty().then_some(v)
    }

    fn value(at: &mut &[u8], depth: usize) -> Option<Cbor> {
        if depth > MOST_DEPTH {
            return None;
        }
        let (&first, rest) = at.split_first()?;
        *at = rest;
        let (major, info) = (first >> 5, first & 0x1f);
        if major == 7 {
            return Some(match info {
                25 => Cbor::Num(half(take(at, 2)? as u16)),
                26 => Cbor::Num(f64::from(f32::from_bits(take(at, 4)? as u32))),
                27 => Cbor::Num(f64::from_bits(take(at, 8)?)),
                0..=23 => Cbor::Other,
                24 => {
                    take(at, 1)?;
                    Cbor::Other
                }
                _ => return None,
            });
        }
        let n = match info {
            0..=23 => u64::from(info),
            24 => take(at, 1)?,
            25 => take(at, 2)?,
            26 => take(at, 4)?,
            27 => take(at, 8)?,
            _ => return None,
        };
        // Each item takes a byte at least, so a list can't hold more than the bytes left; room is set aside for a few
        // only, so a damaged count costs nothing before it's found out.
        let count = |n: u64, at: &[u8]| usize::try_from(n).ok().filter(|n| *n <= at.len());
        Some(match major {
            0 => Cbor::Int(i128::from(n)),
            1 => Cbor::Int(-1 - i128::from(n)),
            2 | 3 => {
                let len = count(n, at)?;
                let (bytes, rest) = (at.get(..len)?, at.get(len..)?);
                *at = rest;
                if major == 2 { Cbor::Bytes } else { Cbor::Text(std::str::from_utf8(bytes).ok()?.to_owned()) }
            }
            4 => {
                let len = count(n, at)?;
                let mut list = Vec::with_capacity(len.min(64));
                for _ in 0..len {
                    list.push(Cbor::value(at, depth + 1)?);
                }
                Cbor::List(list)
            }
            5 => {
                let len = count(n.checked_mul(2)?, at)? / 2;
                let mut map = Vec::with_capacity(len.min(64));
                for _ in 0..len {
                    let k = Cbor::value(at, depth + 1)?;
                    map.push((k, Cbor::value(at, depth + 1)?));
                }
                Cbor::Map(map)
            }
            // Tags aren't in a shape.
            _ => return None,
        })
    }

    /// A map's value at a text key.
    fn get(&self, key: &str) -> Option<&Cbor> {
        let Cbor::Map(map) = self else { return None };
        map.iter().find(|(k, _)| matches!(k, Cbor::Text(t) if t == key)).map(|(_, v)| v)
    }

    fn list(&self) -> Option<&[Cbor]> {
        if let Cbor::List(l) = self { Some(l) } else { None }
    }

    fn int(&self) -> Option<i128> {
        match *self {
            Cbor::Int(n) => Some(n),
            Cbor::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => Some(n as i128),
            _ => None,
        }
    }

    fn num(&self) -> Option<f64> {
        match *self {
            Cbor::Num(n) => Some(n),
            Cbor::Int(n) => Some(n as f64),
            _ => None,
        }
    }
}

/// The next `bytes` bytes as a number, high byte first (CBOR's order).
fn take(at: &mut &[u8], bytes: usize) -> Option<u64> {
    let (head, rest) = (at.get(..bytes)?, at.get(bytes..)?);
    *at = rest;
    Some(head.iter().fold(0u64, |n, b| (n << 8) | u64::from(*b)))
}

/// A 16-bit float's value.
fn half(bits: u16) -> f64 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f64::from(bits & 0x3ff);
    sign * match exp {
        0 => frac * 2f64.powi(-24),
        31 => {
            if frac == 0.0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => (1.0 + frac / 1024.0) * 2f64.powi(exp - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.XferShape` file around a CBOR body, packed as Serum packs it (zstd), the way the test files are made.
    fn xfershape(product: &str, body: &[u8]) -> Vec<u8> {
        let header = format!(r#"{{"fileType":"XferShape","product":"{product}","version":4.0}}"#);
        let packed = ruzstd::encoding::compress_to_vec(body, ruzstd::encoding::CompressionLevel::Fastest);
        let mut out = MAGIC.to_vec();
        out.extend((header.len() as u64).to_le_bytes());
        out.extend(header.as_bytes());
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(2u32.to_le_bytes());
        out.extend(packed);
        out
    }

    fn text(t: &str) -> Vec<u8> {
        let mut out = vec![0x60 | t.len() as u8];
        out.extend(t.as_bytes());
        out
    }

    fn floats(vals: &[f64]) -> Vec<u8> {
        let mut out = vec![0x80 | vals.len() as u8];
        for v in vals {
            out.push(0xfb);
            out.extend(v.to_be_bytes());
        }
        out
    }

    /// The `LFO` map's body, as Serum 2 writes it: one more point in each list than `numPoints` (always 1, 1, 0.5).
    fn body(xs: &[f64], ys: &[f64], curves: &[f64]) -> Vec<u8> {
        let extra = |v: &[f64], last: f64| v.iter().copied().chain([last]).collect::<Vec<_>>();
        let mut out = vec![0xa1];
        out.extend(text("LFO"));
        out.push(0xa4);
        out.extend(text("curveVals"));
        out.extend(floats(&extra(curves, 0.5)));
        out.extend(text("numPoints"));
        out.push(xs.len() as u8);
        out.extend(text("xVals"));
        out.extend(floats(&extra(xs, 1.0)));
        out.extend(text("yVals"));
        out.extend(floats(&extra(ys, 1.0)));
        out
    }

    fn values(s: &Shape) -> Vec<(f64, f64, f64)> {
        s.points.iter().map(|p| (p.x, (p.value * 1e4).round() / 1e4, (p.bend * 1e4).round() / 1e4)).collect()
    }

    #[test]
    fn a_serum_2_shape_is_read_upright_and_joined_back_to_its_start() {
        // Basic\Triangle: bottom at the start, top half way; the line back down to the start's value is added.
        let raw = xfershape("Serum2", &body(&[0.0, 0.5], &[1.0, 0.0], &[0.5, 0.5]));
        let s = read("Shapes/Triangle.XferShape", &raw).remove(0);
        assert_eq!(s.product, "Serum 2");
        assert_eq!(values(&s), [(0.0, 0.0, 0.0), (0.5, 1.0, 0.0), (1.0, 0.0, 0.0)]);
        // Basic\Saw Down Curved: up at once, then down, early (fast first).
        let raw = xfershape("Serum2FX", &body(&[0.0, 0.0], &[1.0, 0.0], &[0.5, 0.2958]));
        let s = read("x.xfershape", &raw).remove(0);
        assert_eq!(s.product, "Serum 2 FX");
        assert_eq!(values(&s), [(0.0, 0.0, 0.0), (0.0, 1.0, -0.4084), (1.0, 0.0, 0.0)]);
    }

    #[test]
    fn a_last_point_at_the_end_isnt_joined_again_and_minus_zero_stays_in_order() {
        // Rhythmic\HalfBar Backbeat 1 starts with a jump on 0, saved as 0 then -0.
        let raw = xfershape("Serum2", &body(&[0.0, -0.0, 0.5, 1.0], &[1.0, 0.25, 0.0, 0.5], &[0.36, 0.39, 0.5, 0.5]));
        let s = read_xfershape(&raw).unwrap();
        let xs: Vec<f64> = s.points.iter().map(|p| p.x).collect();
        assert_eq!(xs, [0.0, 0.0, 0.5, 1.0]);
        assert!(s.points.iter().all(|p| p.x.is_sign_positive()));
        assert_eq!(s.points[1].value, 0.75);
    }

    /// A Serum 1 shape file: bends, across and value in pixels, then the count and 7 bytes of leftovers.
    fn shp(points: &[(f64, f64, f64)]) -> Vec<u8> {
        let mut lists = [[0.0f64; 64]; 4];
        for (i, (x, y, c)) in points.iter().enumerate() {
            (lists[0][i], lists[1][i], lists[2][i]) = (*c, *x, *y);
        }
        let mut out: Vec<u8> = lists.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        out.push(points.len() as u8);
        out.extend([0xff, 0xbe, 0xff, 0x07, 0x5a, 0x67, 0x23]);
        out
    }

    #[test]
    fn a_serum_1_shape_is_read_from_its_pixels() {
        // Sidechain\SC1.shp: three points, the last at the end.
        let raw = shp(&[(0.0, 154.0, 0.6458), (194.0, 35.0, 0.4208), (388.0, 0.0, 0.5)]);
        let s = read("Sidechain/SC1.SHP", &raw).remove(0);
        assert_eq!(s.product, "Serum");
        assert_eq!(values(&s), [(0.0, 0.3583, 0.2916), (0.5, 0.8542, -0.1584), (1.0, 1.0, 0.0)]);
    }

    /// A graph drawn: its number and its points in pixels, with their bends.
    type Graph<'a> = (usize, &'a [(f64, f64, f64)]);

    /// An LFOTool preset with `graphs` drawn (graph number, points in pixels and bends) and the rest as LFOTool starts
    /// them.
    fn lfotool(graphs: &[Graph<'_>]) -> Vec<u8> {
        let mut lists = vec![[0.0f64; 64]; 36];
        let mut counts = [2u8; 12];
        for g in 0..12 {
            // Saved as 32-bit numbers, so "0.5" is a hair under.
            (lists[g][0], lists[g][1]) = (0.4999999701976776, 0.4999999701976776);
            (lists[12 + g][1], lists[24 + g][1]) = (194.0, 120.0);
        }
        for (g, points) in graphs {
            for (i, (x, y, c)) in points.iter().enumerate() {
                (lists[*g][i], lists[12 + g][i], lists[24 + g][i]) = (*c, *x, *y);
            }
            counts[*g] = points.len() as u8;
        }
        let mut out = b"CcnK\0\0\x49\xb4FPCh\0\0\0\x01XFRa\0\0\0\x01\0\0\0\x01".to_vec();
        out.resize(56, 0);
        out.extend(18816u32.to_be_bytes());
        out.extend(lists.iter().flatten().flat_map(|v| v.to_le_bytes()));
        let mut settings = [0u8; 384];
        settings[352..364].copy_from_slice(&counts);
        out.extend(settings);
        out
    }

    #[test]
    fn an_lfotool_preset_gives_the_graphs_drawn_in_it() {
        // Sidechain\Sidechain-1.fxp: graph 1 is SC1, the other eleven as LFOTool starts them.
        let raw = lfotool(&[(0, &[(0.0, 154.0, 0.6458), (194.0, 35.0, 0.4208), (388.0, 0.0, 0.5)])]);
        let shapes = read("Sidechain-1.FXP", &raw);
        assert_eq!(shapes.len(), 1);
        assert_eq!((shapes[0].product.as_str(), shapes[0].part.as_str()), ("LFOTool", "Graph 1"));
        assert_eq!(values(&shapes[0]), [(0.0, 0.3583, 0.2916), (0.5, 0.8542, -0.1584), (1.0, 1.0, 0.0)]);
        // A V drawn by hand is kept, though it starts as LFOTool's own does.
        let v = lfotool(&[(0, &[(0.0, 0.0, 0.5), (194.0, 180.0, 0.5)])]);
        assert_eq!(read_lfotool(&v).len(), 1);
        // Graph 12 drawn too, and nothing at all.
        let two =
            lfotool(&[(0, &[(0.0, 0.0, 0.5), (388.0, 240.0, 0.5)]), (11, &[(0.0, 240.0, 0.5), (97.0, 0.0, 0.3)])]);
        let parts: Vec<String> = read_lfotool(&two).into_iter().map(|s| s.part).collect();
        assert_eq!(parts, ["Graph 1", "Graph 12"]);
        assert_eq!(read_lfotool(&lfotool(&[])), []);
        // Another plugin's preset, or one cut short.
        let mut serum = raw.clone();
        serum[16..20].copy_from_slice(b"XfsX");
        assert_eq!(read_lfotool(&serum), []);
        assert_eq!(read_lfotool(&raw[..raw.len() - 1]), []);
    }

    #[test]
    fn what_isnt_a_shape_gives_none() {
        let good = xfershape("Serum2", &body(&[0.0, 0.5], &[1.0, 0.0], &[0.5, 0.5]));
        assert_eq!(read("a.XferShape", &good).len(), 1);
        // Any shorter file (cut anywhere), another ending, or another file type.
        for cut in 0..good.len() {
            assert_eq!(read("a.XferShape", &good[..cut]), [], "cut at {cut}");
        }
        assert_eq!(read("a.fxp", &good), []);
        assert_eq!(read("a.txt", &good), []);
        let other = String::from_utf8_lossy(&good).replace("XferShape", "XferPreset");
        assert_eq!(read_xfershape(other.as_bytes()), None);
        // numPoints more than the lists hold.
        let mut lying = body(&[0.0, 0.5], &[1.0, 0.0], &[0.5, 0.5]);
        let at = lying.windows(10).position(|w| w == text("numPoints").as_slice()).unwrap() + 10;
        lying[at] = 9;
        assert_eq!(read_xfershape(&xfershape("Serum2", &lying)), None);
        // A Serum 1 file of the wrong size, no points, or too many.
        let s = shp(&[(0.0, 0.0, 0.5), (388.0, 240.0, 0.5)]);
        assert!(read_shp(&s).is_some());
        assert_eq!(read_shp(&s[..2055]), None);
        let mut none = s.clone();
        none[2048] = 0;
        assert_eq!(read_shp(&none), None);
        none[2048] = 65;
        assert_eq!(read_shp(&none), None);
        // A shape with a number that isn't one.
        assert_eq!(read_shp(&shp(&[(f64::NAN, 0.0, 0.5)])), None);
    }

    #[test]
    fn cbor_that_says_more_than_it_holds_or_nests_too_deep_isnt_read() {
        // A list of 2^40 items in 9 bytes.
        assert_eq!(Cbor::parse(&[0x9b, 0, 0, 1, 0, 0, 0, 0, 0]), None);
        // A text longer than what's left.
        assert_eq!(Cbor::parse(&[0x65, b'a']), None);
        // Lists in lists, deeper than allowed.
        assert_eq!(Cbor::parse(&[0x81; 40].iter().copied().chain([0x00]).collect::<Vec<_>>()), None);
        // A list of unknown length.
        assert_eq!(Cbor::parse(&[0x9f, 0x00, 0xff]), None);
        // Half, single and double floats, and leftovers.
        assert_eq!(Cbor::parse(&[0xf9, 0x3c, 0x00]), Some(Cbor::Num(1.0)));
        assert_eq!(Cbor::parse(&[0xfa, 0x3f, 0x00, 0x00, 0x00]), Some(Cbor::Num(0.5)));
        assert_eq!(Cbor::parse(&[0x00, 0x00]), None);
        assert_eq!(Cbor::parse(&[0x38, 0x01]), Some(Cbor::Int(-2)));
    }
}
