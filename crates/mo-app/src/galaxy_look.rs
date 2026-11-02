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
use crate::sounds::SoundList;

/// What makes a star bigger and brighter (its colour always says its type).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LightBy {
    /// All alike.
    Nothing,
    /// The sounds you like most shine (the C# app's look).
    #[default]
    Liked,
    /// The ones used in the most songs.
    Used,
    /// Long sounds (loops, pads) big, one-shots small.
    Length,
    /// The ones added lately.
    Newest,
    /// The fastest.
    Tempo,
}

impl LightBy {
    pub const ALL: [LightBy; 6] =
        [LightBy::Liked, LightBy::Used, LightBy::Length, LightBy::Newest, LightBy::Tempo, LightBy::Nothing];

    /// The name the page shows.
    pub fn name(self) -> &'static str {
        match self {
            LightBy::Nothing => "Nothing",
            LightBy::Liked => "Liked",
            LightBy::Used => "Used in songs",
            LightBy::Length => "Length",
            LightBy::Newest => "Newest",
            LightBy::Tempo => "Tempo",
        }
    }

    pub fn from_name(name: &str) -> Option<LightBy> {
        LightBy::ALL.into_iter().find(|l| l.name() == name)
    }
}

/// How lit each sound in `list` is by `by`, from 0 to 1, by its place in the list: where it ranks among the sounds
/// that have the measure (so one very long loop doesn't leave every other sound dark). A sound without it is 0.
/// When no sound has the measure (or `by` is Nothing) the list is empty: every star is drawn alike.
pub fn light(list: &SoundList, by: LightBy) -> Vec<f32> {
    let n = list.len();
    let value = |i: usize| -> Option<f64> {
        let s = list.sound(i)?;
        match by {
            LightBy::Nothing => None,
            LightBy::Liked => list.liked_share(i).filter(|&v| v > 0.0),
            LightBy::Used => list.used_in(i).filter(|&u| u > 0).map(f64::from),
            LightBy::Length => s.analysis.as_ref().map(|a| a.seconds).filter(|&v| v > 0.0),
            LightBy::Newest => Some(s.added.and_utc().timestamp() as f64),
            LightBy::Tempo => s.analysis.as_ref().and_then(|a| a.bpm).filter(|&v| v > 0.0),
        }
    };
    let mut known: Vec<(f64, usize)> = (0..n).into_par_iter().filter_map(|i| Some((value(i)?, i))).collect();
    if known.is_empty() {
        return Vec::new();
    }
    known.par_sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut out = vec![0.0f32; n];
    let last = known.len().saturating_sub(1).max(1) as f32;
    let mut at = 0;
    while at < known.len() {
        // Equal values share a rank, so the same count lights the same: the middle of the places they take up, so
        // a value most sounds have (one like each, say) sits halfway, not up with the very top.
        let mut end = at;
        while end + 1 < known.len() && known[end + 1].0 == known[at].0 {
            end += 1;
        }
        let rank = if known.len() == 1 { 1.0 } else { (at + end) as f32 / 2.0 / last };
        for &(_, i) in &known[at..=end] {
            out[i] = rank;
        }
        at = end + 1;
    }
    out
}

/// How much bigger a star is drawn for how lit it is (0 to 1): 1 for an unlit one, up to 2.8 (as C#: a well lit
/// star is up to nearly three times as big). The graphics card's shader in `mo-ui` uses the same curve.
pub fn lit_size(lit: f32) -> f32 {
    1.0 + 1.8 * lit.clamp(0.0, 1.0).sqrt()
}

/// How bright a star is for how lit it is (0 to 1): 0.5 for an unlit one, up to 1.4 (as C#: a liked star shines up
/// to nearly three times as bright).
pub fn lit_weight(lit: f32) -> f32 {
    0.5 + 0.9 * lit.clamp(0.0, 1.0).sqrt()
}

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
/// grey); `shown` is the places the filters let through; `light` how lit each is (see [`light`]), which rides in the
/// colour's last byte (0 for a left-out star, 1 to 255 for a shown one). Made on every core.
pub fn pack(
    stars: &[Option<StarPoint>],
    colours: &[i32],
    palette: &[[u8; 3]],
    shown: &[usize],
    light: &[f32],
) -> Vec<u8> {
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
        let lit = light.get(at).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let weight = if want { lit_weight(lit) } else { 0.05 } * twinkle(at);
        let flag = if want { 1 + (lit * 254.0).round() as u8 } else { 0 };
        let mut out = [0u8; STAR_BYTES];
        out[0..4].copy_from_slice(&p.x.to_le_bytes());
        out[4..8].copy_from_slice(&p.y.to_le_bytes());
        out[8..12].copy_from_slice(&[r, g, b, flag]);
        out[12..16].copy_from_slice(&weight.to_le_bytes());
        Some(out)
    };
    let mut bytes: Vec<u8> = Vec::with_capacity(stars.len() * STAR_BYTES);
    for want in [false, true] {
        let part: Vec<[u8; STAR_BYTES]> = stars.par_iter().enumerate().filter_map(|s| star(s, want)).collect();
        bytes.extend_from_slice(part.as_flattened());
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
        let bytes = pack(&stars, &[0, 0, 7], &palette, &[0], &[1.0, 0.0, 0.0]);
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
    fn light_ranks_the_sounds_that_have_the_measure() {
        let day = |y: i32| chrono::NaiveDate::from_ymd_opt(y, 1, 1).and_then(|d| d.and_hms_opt(0, 0, 0)).unwrap();
        let mut sounds: Vec<mo_sounds::Sound> = (0..4).map(|i| mo_sounds::Sound::new(i, "x.wav")).collect();
        for (s, y) in sounds.iter_mut().zip([2020, 2021, 2022, 2022]) {
            s.added = day(y);
        }
        let list = SoundList::new(sounds);
        // The two sounds added in 2022 share the middle of the top two places.
        assert_eq!(light(&list, LightBy::Newest), [0.0, 1.0 / 3.0, 2.5 / 3.0, 2.5 / 3.0]);
        // Nothing lit, or nothing has the measure (use counts aren't in yet): every star alike.
        assert!(light(&list, LightBy::Nothing).is_empty());
        assert!(light(&list, LightBy::Used).is_empty());
        assert_eq!(LightBy::from_name("Used in songs"), Some(LightBy::Used));
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
