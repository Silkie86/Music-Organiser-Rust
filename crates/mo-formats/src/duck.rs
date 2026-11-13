//! Devious Machines' Duck (Duck 1 and Duck 2): its shape files (`.dmlfo`) and the envelopes in its presets
//! (`.preset`, in its `DeviousMachines` data folder). Both are XML; a shape is a `<MultipointEnvelopeData>` with
//! `<node x value curve curveAux shape/>` points, and Duck 2 also saves a picture of the line it draws
//! (`thumbnail`: 129 values, 4-byte floats in base64, one every 1/128 across).
//!
//! Worked out (2026-11-13) from Solomon's `C:\ProgramData\DeviousMachines` (Duck 2's 14 default shapes, Duck 1's 8,
//! and about 30 of their presets), by laying each line's points over its picture:
//!
//! - x runs 0 to 1 in order; a value is kept the right way up (1 at the top). After the last point the line runs on
//!   to the first point's value at the end, drawn as the last point says (the shape repeats).
//! - `shape` 0 is a bent line. Its `curve` (0 to 1, 0.5 straight) bends it the same way whichever way it goes: below
//!   0.5 it stays and then moves (late), above 0.5 it moves early. Every one of 25 lines in the pictures fits
//!   `t' = a t / (1 + (a - 1) t)` (to about 1%), with `ln a = 2.15 * ln(curve / (1 - curve))`.
//! - `shape` 1 and 9 are S lines (1 a steep S, its steepness set by `curve`; 9 a smooth one, like half a sine).
//!   `shape` 7 bends by `curve` and `curveAux` together, in a way not yet worked out; 2 is seen once, in Duck 1.
//!   For these, a line is taken from the picture when there is one (an S is split at its middle into two bent
//!   halves); without one, an S is drawn as two halves bent a fixed amount and the others as `shape` 0.
//! - `curveAux` (Duck 2 only) changes nothing for `shape` 0.
//! - A Duck 2 preset keeps one envelope for each band of its crossover (`Duck2EnvelopeData`, always three); band 2
//!   (and 3, when `crossover3Bands` is 1) is only its own when the bands are unlinked and the crossover is on. Its `MultipointEnvelopeData __id="curve"` is left
//!   over from Duck 1 and isn't read. A Duck 1 preset has just that one, with no picture.
//! - `<lfoSettings start end offset/>` has been 0, 1 and 0 in every file seen, so it isn't used.

use crate::serum::{Shape, closed_with};

/// Goes up by one whenever what's read from the same files changes (rule 7).
pub const VERSION: u32 = 1;

/// The biggest file read: a preset is a few KB (the biggest seen is 28 KB).
const MOST_BYTES: usize = 4 << 20;
/// The most points a shape is read with.
const MOST_POINTS: usize = 1024;
/// How many values a picture holds, one every 1/128 across.
const THUMBNAIL_VALUES: usize = 129;
/// How far each half of an S bends when there's no picture to measure it on.
const S_BEND: f64 = 0.5;
/// `ln a` per `ln(curve / (1 - curve))`, fitted on 25 lines of Duck 2's pictures (each 2.05 to 2.24).
const CURVE_POWER: f64 = 2.15;

/// The shapes in a Duck file, by its name's ending (`.dmlfo`, or a `.preset` in Duck's folder): none for another
/// file or one that can't be read.
pub fn read(path: &str, raw: &[u8]) -> Vec<Shape> {
    if !is_shape_file(path) || raw.len() > MOST_BYTES {
        return Vec::new();
    }
    let Ok(text) = std::str::from_utf8(raw) else { return Vec::new() };
    let Ok(doc) = roxmltree::Document::parse(text) else { return Vec::new() };
    let root = doc.root_element();
    if path.to_lowercase().ends_with(".dmlfo") {
        return if root.has_tag_name("MultipointEnvelopeData") {
            envelope(root, String::new()).into_iter().collect()
        } else {
            Vec::new()
        };
    }
    if !root.has_tag_name("state") {
        return Vec::new();
    }
    let Some(data) = root.children().find(|c| c.has_tag_name("data")) else { return Vec::new() };
    let duck2 = data.children().find(|c| c.has_tag_name("Duck2EnvelopeData"));
    if let Some(duck2) = duck2 {
        // Duck 2's own envelopes only: the Duck 1 one beside them is a leftover.
        let Some(bands) = duck2.children().find(|c| c.has_tag_name("MultibandEnvelopeData")) else { return Vec::new() };
        envelopes(bands)
            .take(bands_used(root))
            .enumerate()
            .filter_map(|(i, e)| envelope(e, format!("Band {}", i + 1)))
            .collect()
    } else {
        envelopes(data)
            .find(|e| e.attribute("__id") == Some("curve"))
            .and_then(|e| envelope(e, "Envelope".into()))
            .into_iter()
            .collect()
    }
}

/// Whether a file's name says it may hold Duck shapes ([`read`]): a `.dmlfo`, or a `.preset` in Duck's own folder
/// (`DeviousMachines/Duck` or `Duck 2`: other plugins, Devious Machines' own among them, name theirs `.preset` too).
pub fn is_shape_file(path: &str) -> bool {
    let lower = path.to_lowercase().replace('\\', "/");
    lower.ends_with(".dmlfo")
        || (lower.ends_with(".preset")
            && (lower.contains("/deviousmachines/duck/") || lower.contains("/deviousmachines/duck 2/")))
}

/// The envelopes right inside `n`.
fn envelopes<'a, 'i>(n: roxmltree::Node<'a, 'i>) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    n.children().filter(|c| c.has_tag_name("MultipointEnvelopeData"))
}

/// How many of a Duck 2 preset's band envelopes it plays: one when they're linked (every band plays the first) or the
/// crossover is off, else two, or three when it's split three ways (`crossover3Bands`).
fn bands_used(state: roxmltree::Node) -> usize {
    let data =
        |id: &str| state.descendants().find(|n| n.attribute("__id") == Some(id)).and_then(|n| n.attribute("value"));
    let crossover = state
        .descendants()
        .find(|n| n.has_tag_name("PARAM") && n.attribute("id") == Some("crossoverMode"))
        .and_then(|n| n.attribute("value")?.parse::<f64>().ok());
    if data("unlinkEnvelopes") != Some("1") || crossover.is_none_or(|m| m == 0.0) {
        1
    } else if data("crossover3Bands") == Some("1") {
        3
    } else {
        2
    }
}

/// One point of Duck's.
#[derive(Clone, Copy, Debug)]
struct Node {
    x: f64,
    value: f64,
    curve: f64,
    shape: u32,
}

/// One envelope, as `part` of its file.
fn envelope(e: roxmltree::Node, part: String) -> Option<Shape> {
    let nodes: Vec<Node> = e
        .children()
        .filter(|c| c.has_tag_name("node"))
        .take(MOST_POINTS + 1)
        .map(|n| {
            let num = |name: &str| n.attribute(name)?.parse::<f64>().ok().filter(|v| v.is_finite());
            Some(Node {
                x: num("x")?,
                value: num("value")?,
                curve: num("curve").unwrap_or(0.5),
                shape: n.attribute("shape").and_then(|s| s.parse().ok()).unwrap_or(0),
            })
        })
        .collect::<Option<_>>()?;
    if nodes.is_empty() || nodes.len() > MOST_POINTS {
        return None;
    }
    let picture = e.attribute("thumbnail").and_then(thumbnail);
    let first = nodes.first()?.value;
    let mut points: Vec<(f64, f64, f64)> = Vec::with_capacity(nodes.len() * 2);
    for (i, a) in nodes.iter().enumerate() {
        let next = match nodes.get(i + 1) {
            Some(b) => (b.x, b.value),
            // The last point runs on to the first value at the end, unless it's already there.
            None if a.x < 1.0 => (1.0, first),
            None => {
                points.push((a.x, 1.0 - a.value, 0.0));
                continue;
            }
        };
        match line(a, next, picture.as_deref()) {
            Line::Bent(bend) => points.push((a.x, 1.0 - a.value, bend)),
            Line::S { middle, first, second } => {
                points.push((a.x, 1.0 - a.value, first));
                let x = (a.x + next.0) / 2.0;
                points.push((x, 1.0 - (a.value + middle * (next.1 - a.value)), second));
            }
        }
    }
    Some(Shape { product: "Duck".into(), part, points: closed_with(points.into_iter(), |b| b)? })
}

/// How a line between two points is drawn in the app's terms.
#[derive(Debug, PartialEq)]
enum Line {
    /// One bend, -1 to 1 (below 0 early, above 0 late).
    Bent(f64),
    /// An S: split at its middle (`middle` of the way up), each half with its own bend.
    S { middle: f64, first: f64, second: f64 },
}

/// The line from `a` to `to` (across, value).
fn line(a: &Node, to: (f64, f64), picture: Option<&[f32]>) -> Line {
    let (width, rise) = (to.0 - a.x, to.1 - a.value);
    if width <= 0.0 || rise.abs() < 1e-6 {
        return Line::Bent(0.0);
    }
    if a.shape == 0 {
        return Line::Bent(bend(a.curve));
    }
    // Measured on the picture, when it has a few values on this line: how far up it is a quarter, half and three
    // quarters across.
    let measured = picture.filter(|_| width * 128.0 >= 6.0).and_then(|p| {
        let up = |t: f64| Some((sample(p, a.x + t * width)? - a.value) / rise);
        Some((up(0.25)?, up(0.5)?, up(0.75)?))
    });
    match measured {
        Some((q1, half, q3)) => {
            let s_like = (q1 < 0.19 && q3 > 0.81) || (q1 > 0.31 && q3 < 0.69);
            if s_like && (0.1..=0.9).contains(&half) {
                let first = from_middle(q1 / half);
                let second = from_middle((q3 - half) / (1.0 - half));
                Line::S { middle: half, first, second }
            } else {
                Line::Bent(from_middle(half))
            }
        }
        None if a.shape == 1 || a.shape == 9 => Line::S { middle: 0.5, first: S_BEND, second: -S_BEND },
        None => Line::Bent(bend(a.curve)),
    }
}

/// The app's bend for a line whose middle is `middle` of the way up (it draws a bend `b` through `0.5 - b / 4`).
fn from_middle(middle: f64) -> f64 {
    if middle.is_finite() { ((0.5 - middle) * 4.0).clamp(-1.0, 1.0) } else { 0.0 }
}

/// The app's bend for a bent line's `curve`: through the same middle, `a / (1 + a)`.
fn bend(curve: f64) -> f64 {
    let c = curve.clamp(1e-6, 1.0 - 1e-6);
    if (c - 0.5).abs() < 0.002 {
        return 0.0;
    }
    let a = (CURVE_POWER * (c / (1.0 - c)).ln()).exp();
    from_middle(a / (1.0 + a))
}

/// The picture's value at `x` (0 to 1), between its two nearest values.
fn sample(picture: &[f32], x: f64) -> Option<f64> {
    let at = x.clamp(0.0, 1.0) * (THUMBNAIL_VALUES - 1) as f64;
    let (i, frac) = (at.floor() as usize, at.fract());
    let a = f64::from(*picture.get(i)?);
    let b = picture.get(i + 1).map_or(a, |v| f64::from(*v));
    Some(a + (b - a) * frac).filter(|v| v.is_finite())
}

/// A picture's values: base64 of 129 4-byte floats, low byte first.
fn thumbnail(text: &str) -> Option<Vec<f32>> {
    let bytes = base64(text)?;
    if bytes.len() != THUMBNAIL_VALUES * 4 {
        return None;
    }
    Some(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// Plain base64 (`A-Z a-z 0-9 + /`, `=` padding), or none if it isn't.
fn base64(text: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    // Line breaks and spaces are passed over, as some writers wrap it.
    let text: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let body = text.strip_suffix(b"==").or_else(|| text.strip_suffix(b"=")).unwrap_or(&text);
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    for chunk in body.chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= digit(*c)? << (18 - 6 * i);
        }
        let got = chunk.len() - 1;
        out.extend(n.to_be_bytes().iter().skip(1).take(got));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(s: &Shape) -> Vec<(f64, f64, f64)> {
        let r = |v: f64| (v * 1e3).round() / 1e3;
        s.points.iter().map(|p| (r(p.x), r(p.value), r(p.bend))).collect()
    }

    /// A picture's text for the values `f` gives across.
    fn picture(f: impl Fn(f64) -> f64) -> String {
        let bytes: Vec<u8> = (0..THUMBNAIL_VALUES).flat_map(|k| (f(k as f64 / 128.0) as f32).to_le_bytes()).collect();
        let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                out.push(table[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
            out.extend(std::iter::repeat_n('=', 3 - chunk.len()));
        }
        out
    }

    #[test]
    fn a_duck_shape_is_read_upright_and_runs_on_to_its_start() {
        // Duck 2's Defaults\9.dmlfo: a triangle.
        let tri = r#"<?xml version="1.0" encoding="UTF-8"?>
<MultipointEnvelopeData thumbnail="">
  <node x="0.0" value="0.0" curve="0.5" curveAux="0.5" shape="0"/>
  <node x="0.5" value="1.0" curve="0.5" curveAux="0.5" shape="0"/>
  <node x="1.0" value="0.0" curve="0.5" curveAux="0.5" shape="0"/>
  <lfoSettings start="0.0" end="1.0" offset="0.0"/>
</MultipointEnvelopeData>"#;
        let s = read("Duck 2/shapes/Defaults/9.dmlfo", tri.as_bytes()).remove(0);
        assert_eq!((s.product.as_str(), s.part.as_str()), ("Duck", ""));
        assert_eq!(values(&s), [(0.0, 0.0, 0.0), (0.5, 1.0, 0.0), (1.0, 0.0, 0.0)]);
        // Defaults\3.dmlfo's rise is bent early (curve 0.6446); a last point short of the end runs on to the start.
        let rise = r#"<MultipointEnvelopeData><node x="0.0" value="0.0" curve="0.6446" shape="0"/>
            <node x="0.5" value="1.0" curve="0.5" shape="0"/></MultipointEnvelopeData>"#;
        let s = read("3.DMLFO", rise.as_bytes()).remove(0);
        assert_eq!(values(&s), [(0.0, 0.0, -1.0), (0.5, 1.0, 0.0), (1.0, 0.0, 0.0)]);
    }

    #[test]
    fn a_bent_lines_curve_becomes_the_apps_bend_through_the_same_middle() {
        assert_eq!(bend(0.5), 0.0);
        // Default.preset's duck, curve 0.2962: its picture is 0.14 up half way, later than the app's fullest bend
        // (0.25 up half way), so it's that.
        assert_eq!(bend(0.2962), 1.0);
        // Keep The Transient, curve 0.4215: 0.34 up half way in its picture (late); Boing's 0.5325, 0.57 (early).
        assert!((0.55..0.75).contains(&bend(0.4215)), "{}", bend(0.4215));
        assert!((-0.35..-0.2).contains(&bend(0.5325)), "{}", bend(0.5325));
        assert_eq!((bend(0.0), bend(1.0)), (1.0, -1.0));
    }

    #[test]
    fn an_s_line_is_split_at_its_middle_and_measured_on_the_picture() {
        // Defaults\11.dmlfo: a smooth S up and back down (shape 9), with its picture.
        let half_sine = |x: f64| {
            if x <= 0.5 {
                (1.0 - (std::f64::consts::PI * x * 2.0).cos()) / 2.0
            } else {
                (1.0 + (std::f64::consts::PI * (x - 0.5) * 2.0).cos()) / 2.0
            }
        };
        let xml = format!(
            r#"<MultipointEnvelopeData thumbnail="{}"><node x="0.0" value="0.0" curve="1.0" shape="9"/>
            <node x="0.5" value="1.0" curve="1.0" shape="9"/><node x="1.0" value="0.0" curve="1.0" shape="9"/>
            </MultipointEnvelopeData>"#,
            picture(half_sine)
        );
        let s = read("11.dmlfo", xml.as_bytes()).remove(0);
        let v = values(&s);
        assert_eq!(v.len(), 5, "{v:?}");
        assert_eq!((v[1].0, v[1].1), (0.25, 0.5));
        // Each half: the first eases in (late), the second out (early); a quarter of a sine is 0.29 up half way.
        let first = from_middle((1.0 - (std::f64::consts::FRAC_PI_4).cos()) / 2.0 / 0.5);
        assert!((v[0].2 - first).abs() < 0.02 && v[0].2 > 0.0, "{v:?}");
        assert!((v[1].2 + first).abs() < 0.02, "{v:?}");
        // Without a picture an S is still two halves.
        let bare = r#"<MultipointEnvelopeData><node x="0.0" value="1.0" shape="9"/><node x="0.5" value="0.0" shape="9"/>
            </MultipointEnvelopeData>"#;
        let v = values(&read("See Saw.dmlfo", bare.as_bytes()).remove(0));
        assert_eq!(v, [(0.0, 1.0, 0.5), (0.25, 0.5, -0.5), (0.5, 0.0, 0.5), (0.75, 0.5, -0.5), (1.0, 1.0, 0.0)]);
    }

    #[test]
    fn a_presets_envelopes_are_its_bands_or_duck_1s_one() {
        let env = |v: &str| {
            format!(
                r#"<MultipointEnvelopeData thumbnail=""><node x="0.0" value="{v}" curve="0.5" shape="0"/>
                <node x="0.5" value="1.0" curve="0.5" shape="0"/></MultipointEnvelopeData>"#
            )
        };
        let duck2 = |unlink: &str, crossover: &str| {
            format!(
                r#"<state><parameters><PARAM id="crossoverMode" value="{crossover}"/></parameters><data>
                <unlinkEnvelopes value="{unlink}" __id="unlinkEnvelopes"/><crossover3Bands value="1" __id="crossover3Bands"/>
                <Duck2EnvelopeData teamIndex="0" __id="envelopeData"><MultibandEnvelopeData>{}{}{}</MultibandEnvelopeData>
                </Duck2EnvelopeData>{}</data></state>"#,
                env("0.0"),
                env("0.5"),
                env("0.25"),
                env("0.75").replace("<MultipointEnvelopeData", r#"<MultipointEnvelopeData __id="curve""#)
            )
        };
        let parts = |xml: String| -> Vec<String> {
            read("C:/ProgramData/DeviousMachines/Duck 2/Library/Presets/main/Pump.preset", xml.as_bytes())
                .into_iter()
                .map(|s| s.part)
                .collect()
        };
        assert_eq!(parts(duck2("1", "1.0")), ["Band 1", "Band 2", "Band 3"]);
        assert_eq!(parts(duck2("0", "1.0")), ["Band 1"], "linked: every band is band 1's");
        assert_eq!(parts(duck2("1", "0.0")), ["Band 1"], "no crossover: one band");
        let two = duck2("1", "1.0").replace(r#"crossover3Bands value="1""#, r#"crossover3Bands value="0""#);
        assert_eq!(parts(two), ["Band 1", "Band 2"], "split in two");
        // A Duck 2 preset with no bands doesn't fall back on its Duck 1 leftover.
        let no_bands = duck2("1", "1.0");
        let cut =
            no_bands.find("<MultibandEnvelopeData>").unwrap()..no_bands.find("</MultibandEnvelopeData>").unwrap() + 23;
        let mut no_bands = no_bands;
        no_bands.replace_range(cut, "");
        assert_eq!(parts(no_bands), Vec::<String>::new());
        // Duck 1: just the curve, no picture.
        let duck1 = format!(
            "<state><parameters/><data>{}</data></state>",
            env("0.0").replace("<MultipointEnvelopeData thumbnail=\"\"", r#"<MultipointEnvelopeData __id="curve""#)
        );
        assert_eq!(parts(duck1.clone()), ["Envelope"]);
        // Another plugin's .preset, outside Duck's folder, isn't read.
        assert_eq!(read("C:/Presets/Lead.preset", duck1.as_bytes()), []);
        assert!(!is_shape_file("D:\\Other\\Lead.preset"));
        assert!(!is_shape_file("C:\\ProgramData\\DeviousMachines\\Infiltrator\\Library\\Presets\\X.preset"));
        assert!(is_shape_file(
            "C:\\ProgramData\\DeviousMachines\\Duck\\Library\\Presets\\main\\Rhythms\\Clave 01.preset"
        ));
    }

    #[test]
    fn what_isnt_a_duck_shape_gives_none() {
        let good = br#"<MultipointEnvelopeData><node x="0.0" value="0.0" shape="0"/><node x="0.5" value="1.0" shape="0"/></MultipointEnvelopeData>"#;
        assert_eq!(read("a.dmlfo", good).len(), 1);
        for cut in 0..good.len() {
            let _ = read("a.dmlfo", &good[..cut]);
        }
        assert_eq!(read("a.dmlfo", br#"<MultipointEnvelopeData><node x="a" value="0"/></MultipointEnvelopeData>"#), []);
        assert_eq!(read("a.dmlfo", br#"<MultipointEnvelopeData/>"#), []);
        assert_eq!(read("a.dmlfo", b"not xml"), []);
        assert_eq!(read("a.xml", good), []);
        // A picture that isn't 129 values, or isn't base64, is passed over.
        assert_eq!(thumbnail("AAAA"), None);
        assert_eq!(base64("A"), None);
        assert_eq!(base64("AAA="), Some(vec![0, 0]));
        assert_eq!(base64("TWFu"), Some(b"Man".to_vec()));
        assert_eq!(base64("TW\nFu\r\n"), Some(b"Man".to_vec()), "wrapped");
        assert_eq!(base64("AA==="), None);
    }
}
