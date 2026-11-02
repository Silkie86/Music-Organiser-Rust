//! How the galaxy looks when the graphics card draws it, without a window: the numbers its shaders are given. It's
//! the C# app's look:
//!
//! - every star is stamped as a tiny soft dot onto a layer where the light adds up;
//! - a quarter-size copy of that layer, blurred, is the glow round the crowded places;
//! - one last step squeezes the brightness, so a crowd stays vivid and keeps its colour instead of burning white.
//!
//! How many stars are on the view decides the rest. Far out they're only painted that way. Close up each one is also
//! drawn crisply, a bright core in a soft halo, with the stars the filters leave out kept as faint dust. In between
//! the two blend.

use mo_sounds::StarPoint;
use rayon::prelude::*;

use crate::galaxy_view::View;

/// Below this many stars on the view each is drawn crisply; above the upper one they're only painted (as C#).
const CRISP: f32 = 1_500.0;
const PAINTED: f32 = 5_000.0;
/// Stamps are stored at a fraction of their brightness, so a crowd can pile onto one pixel before the layer runs
/// out of room; the last step scales back up.
pub const STAMP: f32 = 1.0 / 6.0;
/// The bytes each star takes on the graphics card: where it is (two f32), its colour (four bytes, the last 255 when
/// the filters let it through) and its weight (an f32).
pub const STAR_BYTES: usize = 16;

/// What the shaders need for one view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// How painted the view is: 1 far out, 0 close up.
    pub painted: f32,
    /// How strongly the stamped stars show, and the glow.
    pub core: f32,
    pub glow: f32,
    /// How quickly the brightness squeezes.
    pub gain: f32,
    /// A crisp star's core radius, in points.
    pub radius: f32,
}

/// A smooth step from 0 at `a` to 1 at `b`.
fn smooth(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The look for `view` with `in_view` shown stars on it.
pub fn look(view: &View, in_view: usize) -> Look {
    let n = in_view as f32;
    let painted = smooth(CRISP, PAINTED, n);
    let area = (view.width * view.height).max(1.0);
    // The more stars per point, the gentler the squeeze, so a busy view isn't all white.
    let gain = (1.7 / (n * 9.0 / area).max(1.0)).clamp(0.35, 1.7);
    // Stars grow as the view flies in, but never so big that a crowd of them runs together.
    let grow = 0.75 + 0.25 * view.zoom.min(30.0).sqrt();
    let room = ((area / n.max(1.0)).sqrt() * 0.22).clamp(1.6, 9.0);
    Look {
        painted,
        core: painted / STAMP,
        // A quarter-size stamp covers a sixteenth of the pixels; the 0.9 is C#'s.
        glow: (0.35 + 0.65 * painted) / STAMP * 0.9,
        gain,
        radius: (1.9 * grow).clamp(1.6, room),
    }
}

/// The part of the map on show, with a little room round it: left, top, right, bottom.
pub fn on_view(view: &View) -> (f32, f32, f32, f32) {
    let (l, t) = view.to_map(0.0, 0.0);
    let (r, b) = view.to_map(view.width, view.height);
    (l, t, r, b)
}

/// A little steady variety in brightness, so a crowd of one type isn't flat (as C#'s twinkle).
fn twinkle(at: usize) -> f32 {
    let h = (at as u32).wrapping_mul(2_654_435_761) >> 24;
    0.6 + 0.4 * h as f32 / 255.0
}

/// Every star on the map as the graphics card reads it ([`STAR_BYTES`] each), the ones the filters leave out first so
/// the crisp ones are drawn over them. `colours` is each star's place in `palette` (anything else is the last colour,
/// grey); `shown` is the places the filters let through. Made on every core.
pub fn pack(stars: &[Option<StarPoint>], colours: &[i32], palette: &[[u8; 3]], shown: &[usize]) -> Vec<u8> {
    let mut on = vec![false; stars.len()];
    for &at in shown {
        if let Some(o) = on.get_mut(at) {
            *o = true;
        }
    }
    let grey = palette.last().copied().unwrap_or([110, 110, 110]);
    let star = |(at, p): (usize, &Option<StarPoint>), want: bool| -> Option<[u8; STAR_BYTES]> {
        let p = (*p)?;
        if on[at] != want {
            return None;
        }
        let c = colours.get(at).and_then(|&i| usize::try_from(i).ok()).and_then(|i| palette.get(i)).copied();
        let [r, g, b] = c.unwrap_or(grey);
        let weight = if want { 0.5 } else { 0.05 } * twinkle(at);
        let mut out = [0u8; STAR_BYTES];
        out[0..4].copy_from_slice(&p.x.to_le_bytes());
        out[4..8].copy_from_slice(&p.y.to_le_bytes());
        out[8..12].copy_from_slice(&[r, g, b, if want { 255 } else { 0 }]);
        out[12..16].copy_from_slice(&weight.to_le_bytes());
        Some(out)
    };
    let mut bytes: Vec<u8> = Vec::with_capacity(stars.len() * STAR_BYTES);
    for want in [false, true] {
        let part: Vec<[u8; STAR_BYTES]> = stars.par_iter().enumerate().filter_map(|s| star(s, want)).collect();
        bytes.extend(part.iter().flatten());
    }
    bytes
}

/// The camera as the star shaders read it (8 f32, 32 bytes) for a layer `width` by `height` pixels that shows `view`
/// at `scale` pixels per point: the map point in the middle, map units to the layer's clip space (x, then y, which
/// runs up), one pixel in clip space, and the size of a stamp's blur (`sigma`, in the layer's pixels) and its
/// strength.
pub fn camera(view: &View, scale: f32, width: u32, height: u32, sigma: f32, strength: f32) -> [f32; 8] {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    // The layer may be smaller than the view (the glow's quarter size): it covers the same view all the same.
    let unit = view.unit() * scale;
    let (sx, sy) = (w / (view.width * scale).max(1.0), h / (view.height * scale).max(1.0));
    [view.cx, view.cy, unit * sx * 2.0 / w, -unit * sy * 2.0 / h, 2.0 / w, 2.0 / h, sigma, strength]
}

/// Little-endian bytes of `values`, for the graphics card.
pub fn bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        let mut v = View::default();
        (v.width, v.height) = (800.0, 600.0);
        v
    }

    #[test]
    fn far_out_is_painted_and_close_up_is_crisp() {
        let v = view();
        assert_eq!(look(&v, 450_000).painted, 1.0);
        assert_eq!(look(&v, 200).painted, 0.0);
        let between = look(&v, 3_000).painted;
        assert!(between > 0.0 && between < 1.0);
        // A crowded view squeezes gently, a sparse one fully.
        assert!(look(&v, 450_000).gain < look(&v, 2_000).gain);
        assert_eq!(look(&v, 10).gain, 1.7);
    }

    #[test]
    fn stars_grow_as_the_view_flies_in_but_keep_their_room() {
        let mut v = view();
        let far = look(&v, 100).radius;
        v.zoom = 20.0;
        let near = look(&v, 100).radius;
        assert!(near > far);
        // Very crowded: they shrink to keep apart, never below 1.6 points.
        assert_eq!(look(&v, 1_000_000).radius, 1.6);
    }

    #[test]
    fn the_left_out_stars_come_first_and_the_shown_ones_are_marked() {
        let stars = vec![Some(StarPoint { x: 0.25, y: 0.5 }), None, Some(StarPoint { x: 0.75, y: 0.1 })];
        let palette = [[200, 100, 50], [110, 110, 110]];
        let bytes = pack(&stars, &[0, 0, 7], &palette, &[0]);
        assert_eq!(bytes.len(), 2 * STAR_BYTES);
        // The left out one (the third, grey: its colour isn't in the palette).
        assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 0.75);
        assert_eq!(&bytes[8..12], &[110, 110, 110, 0]);
        // Then the shown one, in its colour.
        assert_eq!(f32::from_le_bytes(bytes[16..20].try_into().unwrap()), 0.25);
        assert_eq!(&bytes[24..28], &[200, 100, 50, 255]);
        assert!(
            f32::from_le_bytes(bytes[28..32].try_into().unwrap())
                > f32::from_le_bytes(bytes[12..16].try_into().unwrap())
        );
    }

    #[test]
    fn the_camera_puts_the_middle_of_the_view_in_the_middle() {
        let mut v = view();
        (v.cx, v.cy, v.zoom) = (0.3, 0.6, 2.0);
        let c = camera(&v, 1.0, 800, 600, 0.6, 1.0);
        // A map point's clip place is (point - middle) * unit: the view's corner lands on the clip corner.
        let (l, t) = v.to_map(0.0, 0.0);
        assert!(((l - c[0]) * c[2] + 1.0).abs() < 1e-4);
        assert!(((t - c[1]) * c[3] - 1.0).abs() < 1e-4);
        // The quarter-size layer covers the same part of the map.
        let q = camera(&v, 1.0, 200, 150, 0.6, 1.0);
        assert!(((l - q[0]) * q[2] + 1.0).abs() < 1e-4);
        assert_eq!((q[4], q[5]), (0.01, 2.0 / 150.0));
    }
}
