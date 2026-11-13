//! Vital's LFO shapes: its shape files (`.vitallfo`) and the eight LFOs in each of its presets (`.vital`). Both are
//! JSON; a shape is `{"name", "num_points", "points": [x, y, x, y, ...], "powers": [...], "smooth"}`, and a preset
//! keeps eight of them under `settings.lfos`.
//!
//! Worked out (2026-11-13) from Solomon's `Vital` data folder (Factory\LFOs and his preset packs), with how Vital draws
//! a shape taken from Vital's own source (its line generator, read for the maths only):
//!
//! - x runs 0 to 1 in order; y is kept top down (1 is the bottom: "Saw Up" goes from y 1 to y 0), so value = 1 - y.
//! - `powers[i]` bends the line from point i to the next, as `t' = (e^(p t) - 1) / (e^p - 1)`: 0 is straight, above 0
//!   it stays and then moves (late), below 0 it moves early, whichever way the line goes. The app's bend is a curve
//!   through the same middle: bend = (0.5 - t'(0.5)) * 4, so a power of about ±2.2 is a full bend.
//! - The last point joins the first again at the end, bent by the last point's power (the shape repeats).
//! - `smooth` draws every line as an S (its powers then don't count): it's kept near enough, each line split at its
//!   middle into a late half and an early half ([`as_s`]).
//!
//! An LFO nobody changed in a preset is Vital's own start, a plain triangle (named "Triangle", (0, 1), (0.5, 0),
//! (1, 1), straight): nearly every preset has several, so they're passed over.

use crate::serum::{Shape, closed_with};

/// Goes up by one whenever what's read from the same files changes (rule 7).
pub const VERSION: u32 = 1;

/// The most points a shape is read with (Vital's own limit is 100).
const MOST_POINTS: usize = 1024;

/// The shapes in a Vital file, by its name's ending (`.vitallfo`, or a preset's `.vital`): none for another file or
/// one that can't be read.
pub fn read(path: &str, raw: &[u8]) -> Vec<Shape> {
    let lower = path.to_lowercase();
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(raw) else { return Vec::new() };
    if lower.ends_with(".vitallfo") {
        lfo(&json, String::new()).into_iter().collect()
    } else if lower.ends_with(".vital") {
        let Some(lfos) = json.get("settings").and_then(|s| s.get("lfos")).and_then(|l| l.as_array()) else {
            return Vec::new();
        };
        lfos.iter()
            .enumerate()
            .filter(|(_, l)| !is_untouched(l))
            .filter_map(|(i, l)| lfo(l, format!("LFO {}", i + 1)))
            .collect()
    } else {
        Vec::new()
    }
}

/// Whether a file's name says it may hold Vital shapes ([`read`]).
pub fn is_shape_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".vitallfo") || lower.ends_with(".vital")
}

/// One shape, as `part` of its file.
fn lfo(l: &serde_json::Value, part: String) -> Option<Shape> {
    let n = usize::try_from(l.get("num_points")?.as_u64()?).ok().filter(|n| *n <= MOST_POINTS)?;
    let points = l.get("points")?.as_array()?;
    let powers = l.get("powers")?.as_array()?;
    let num = |v: Option<&serde_json::Value>| v?.as_f64();
    let read: Vec<(f64, f64, f64)> = (0..n)
        .map(|i| Some((num(points.get(2 * i))?, num(points.get(2 * i + 1))?, bend(num(powers.get(i))?))))
        .collect::<Option<_>>()?;
    let smooth = l.get("smooth").and_then(|s| s.as_bool()).unwrap_or(false);
    let read = if smooth { as_s(&read) } else { read };
    Some(Shape { product: "Vital".into(), part, points: closed_with(read.into_iter(), |b| b)? })
}

/// How far each half of a smooth line bends: in late, then out early.
const S_BEND: f64 = 0.5;

/// A smooth shape's lines drawn as an S, near enough: each line is split at its middle, the first half bent late
/// (it eases in) and the second early (it eases out). The last line runs on to the first point's value at the end.
fn as_s(points: &[(f64, f64, f64)]) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::with_capacity(points.len() * 2);
    for (i, &(x, y, _)) in points.iter().enumerate() {
        out.push((x, y, S_BEND));
        let next = match points.get(i + 1) {
            Some(&(nx, ny, _)) => (nx, ny),
            None if x < 1.0 => (1.0, points[0].1),
            None => continue,
        };
        out.push(((x + next.0) / 2.0, (y + next.1) / 2.0, -S_BEND));
    }
    out
}

/// The app's bend for one of Vital's powers: a curve through the same middle.
fn bend(power: f64) -> f64 {
    if power.abs() < 0.01 {
        return 0.0;
    }
    // Far past a full bend, e^p would run out of range; the middle is then 0 or 1 all the same.
    let p = power.clamp(-60.0, 60.0);
    let middle = ((p / 2.0).exp() - 1.0) / (p.exp() - 1.0);
    ((0.5 - middle) * 4.0).clamp(-1.0, 1.0)
}

/// Whether a preset's LFO is Vital's own start: the plain triangle every new preset has.
fn is_untouched(l: &serde_json::Value) -> bool {
    let triangle = [0.0, 1.0, 0.5, 0.0, 1.0, 1.0];
    // Each of the first six numbers in its place: something that isn't a number there isn't the triangle.
    let points: Option<Vec<f64>> =
        l.get("points").and_then(|p| p.as_array()).and_then(|p| p.get(..6)?.iter().map(|v| v.as_f64()).collect());
    let powers_flat = l
        .get("powers")
        .and_then(|p| p.as_array())
        .is_some_and(|p| p.iter().take(3).all(|v| v.as_f64().is_some_and(|v| v == 0.0)));
    l.get("num_points").and_then(|n| n.as_u64()) == Some(3)
        && l.get("name").and_then(|n| n.as_str()).is_none_or(|n| n == "Triangle")
        && !l.get("smooth").and_then(|s| s.as_bool()).unwrap_or(false)
        && points.is_some_and(|p| p[..] == triangle[..])
        && powers_flat
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(s: &Shape) -> Vec<(f64, f64, f64)> {
        let r = |v: f64| (v * 1e4).round() / 1e4;
        s.points.iter().map(|p| (r(p.x), r(p.value), r(p.bend))).collect()
    }

    #[test]
    fn a_vital_shape_is_read_upright_with_its_bends() {
        // Factory\LFOs\Basic\Saw Up.vitallfo, as Vital saves it.
        let saw = br#"{"num_points":3,"points":[0.0,1.0,1.0,0.0,1.0,1.0],"powers":[0.0,0.0,0.0],"smooth":false}"#;
        let s = read("Basic/Saw Up.vitallfo", saw).remove(0);
        assert_eq!((s.product.as_str(), s.part.as_str()), ("Vital", ""));
        assert_eq!(values(&s), [(0.0, 0.0, 0.0), (1.0, 1.0, 0.0), (1.0, 0.0, 0.0)]);
        // Pattern\Side Chain 1: a bend of +4.6 stays then moves (late), -3.4 moves early; past a full bend is kept to 1.
        let side = br#"{"name":"Triangle","num_points":5,"points":[0.0,0.0,0.1063,0.9821,0.3212,0.8679,0.8374,0.0,1.0,0.0],"powers":[-3.4018,4.6339,-2.4107,0.0,0.0],"smooth":false}"#;
        let s = read("Side Chain 1.VITALLFO", side).remove(0);
        let bends: Vec<f64> = s.points.iter().map(|p| (p.bend * 100.0).round() / 100.0).collect();
        assert_eq!(bends, [-1.0, 1.0, -1.0, 0.0, 0.0]);
        assert_eq!(s.points[0].value, 1.0);
        assert!((bend(1.0) - 0.4898).abs() < 1e-3 && (bend(-1.0) + 0.4898).abs() < 1e-3, "{}", bend(1.0));
        assert_eq!((bend(1e9), bend(-1e9)), (1.0, -1.0));
    }

    #[test]
    fn a_presets_lfos_are_read_but_not_the_untouched_triangles() {
        let untouched = r#"{"name":"Triangle","num_points":3,"points":[0.0,1.0,0.5,0.0,1.0,1.0],"powers":[0.0,0.0,0.0],"smooth":false}"#;
        let drawn = r#"{"name":"Mine","num_points":2,"points":[0.0,1.0,0.25,0.0],"powers":[1.0,0.0],"smooth":false}"#;
        let bent = r#"{"name":"Triangle","num_points":3,"points":[0.0,1.0,0.5,0.0,1.0,1.0],"powers":[2.0,0.0,0.0],"smooth":false}"#;
        let preset = format!(
            r#"{{"author":"x","settings":{{"lfos":[{untouched},{drawn},{untouched},{bent},{untouched},{untouched},{untouched},{untouched}]}}}}"#
        );
        let shapes = read("Banana Wob.vital", preset.as_bytes());
        let parts: Vec<&str> = shapes.iter().map(|s| s.part.as_str()).collect();
        assert_eq!(parts, ["LFO 2", "LFO 4"]);
        assert_eq!(values(&shapes[0]), [(0.0, 0.0, 0.4898), (0.25, 1.0, 0.0), (1.0, 0.0, 0.0)]);
    }

    #[test]
    fn what_isnt_a_vital_shape_gives_none() {
        let good = br#"{"num_points":2,"points":[0.0,1.0,0.5,0.0],"powers":[0.0,0.0]}"#;
        assert_eq!(read("a.vitallfo", good).len(), 1);
        for cut in 0..good.len() {
            assert_eq!(read("a.vitallfo", &good[..cut]), [], "cut at {cut}");
        }
        assert_eq!(read("a.json", good), []);
        // More points said than kept, a number that isn't one, no LFOs in a preset.
        assert_eq!(read("a.vitallfo", br#"{"num_points":3,"points":[0.0,1.0,0.5,0.0],"powers":[0,0,0]}"#), []);
        assert_eq!(read("a.vitallfo", br#"{"num_points":1,"points":["a",1.0],"powers":[0]}"#), []);
        assert_eq!(read("a.vital", br#"{"settings":{}}"#), []);
        assert_eq!(read("a.vital", b"not json"), []);
        // A damaged triangle (a word where a number goes) isn't taken for the untouched one.
        let odd = r#"{"name":"Triangle","num_points":3,"points":[null,0.0,1.0,0.5,0.0,1.0,1.0],"powers":[0,0,0]}"#;
        assert!(!is_untouched(&serde_json::from_str(odd).unwrap()));
    }

    #[test]
    fn a_smooth_shape_is_drawn_as_an_s_and_isnt_the_same_as_a_straight_one() {
        let smooth = br#"{"num_points":2,"points":[0.0,1.0,0.5,0.0],"powers":[0.0,0.0],"smooth":true}"#;
        let s = read("Round.vitallfo", smooth).remove(0);
        assert_eq!(
            values(&s),
            [(0.0, 0.0, 0.5), (0.25, 0.5, -0.5), (0.5, 1.0, 0.5), (0.75, 0.5, -0.5), (1.0, 0.0, 0.0)]
        );
        // A last point already at the end has no line after it to split.
        let saw = br#"{"num_points":2,"points":[0.0,1.0,1.0,0.0],"powers":[0.0,0.0],"smooth":true}"#;
        assert_eq!(values(&read("Saw.vitallfo", saw).remove(0)), [(0.0, 0.0, 0.5), (0.5, 0.5, -0.5), (1.0, 1.0, 0.5)]);
    }
}
