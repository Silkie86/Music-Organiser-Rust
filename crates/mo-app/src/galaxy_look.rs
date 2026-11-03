//! How the galaxy looks when the graphics card draws it, without a window: the numbers its shaders are given. It's
//! the C# app's look:
//!
//! - every star is stamped as a tiny soft dot onto a layer where the light adds up;
//! - a quarter-size copy of that layer, blurred, is the glow round the crowded places;
//! - one last step squeezes the brightness, so a crowd stays vivid and keeps its colour instead of burning white.
//!
//! How many stars are on the view decides the rest. Far out they're only painted that way. Close up each one is also
//! drawn as a star, a white-hot core in a soft glow, with the stars the filters leave out kept as faint dust. In
//! between the two blend.
//!
//! With Light by on, the unlit stars are dimmer and smaller, and the best lit ones are drawn as stars even far out
//! (never smaller than [`MIN_LIT`] points), with a bigger glow; the very brightest few get faint four-point spikes.
//! So what Light by picks out shows at every zoom, while the unlit stars still keep the map's shape. Which ones those
//! are goes by their rank ([`lit_order`]), the same on the card and on the processor.

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
/// that have the measure (so one very long loop doesn't leave every other sound dark). A sound without it is 0, and
/// one with it never is (the least lit is only just above), so "above 0" always means "has the measure".
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
        let rank = if known.len() == 1 { 1.0 } else { ((at + end) as f32 / 2.0 / last).max(f32::MIN_POSITIVE) };
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

/// How bright a star is for how lit it is (0 to 1), with Light by on: 0.3 for an unlit one (dimmer than the 0.5 of
/// an all-alike star, so the lit ones stand out), up to 1.5, five times as bright.
pub fn lit_weight(lit: f32) -> f32 {
    0.3 + 1.2 * lit.clamp(0.0, 1.0).sqrt()
}

/// How bright every star is with nothing lit.
pub const ALIKE_WEIGHT: f32 = 0.5;

/// With Light by on, an unlit star is drawn this much the size of an all-alike one, and a fully lit one 2.8 times that
/// ([`lit_size`]), so the lit ones stand out without the map filling up.
pub const UNLIT_SIZE: f32 = 0.7;
/// Far out, a star lit enough to show as a point of its own (it "glints") has a core at least this many points across
/// its radius, up to 1.6 times that for the best lit ([`glint_point`]). In points, so it's the same size on a 4K
/// screen as on any other, and on the processor's picture drawn smaller while the view glides.
pub const MIN_LIT: f32 = 1.6;
/// The shortest a star's spikes are, in points.
pub const SPIKE_MIN: f32 = 10.0;
/// A star's glow (as drawn by both the card and the processor): how bright it starts, how much brighter for a lit
/// star, and its white-hot middle (how wide, as a share of the core's radius squared, and how white).
pub const GLOW: f32 = 0.36;
pub const GLOW_LIT: f32 = 0.3;
pub const HOT: f32 = 0.08;
pub const WHITE: f32 = 0.4;
/// How much of that glow a star has far out, where it's one of thousands and their glows would add up to white.
pub const FAR_GLOW: f32 = 0.2;
/// How far a star's glow reaches far out, as a share of its core's radius (close up it's 2.8, up to twice that lit).
pub const FAR_HALO: f32 = 2.2;
/// Close up, how far a star's glow reaches, as a share of its core's radius (a well lit one's up to twice that), and
/// how softly it falls off (the power of its edge).
pub const HALO: f32 = 2.8;
pub const FALLOFF: f32 = 1.3;
/// How bright the best lit star is ([`lit_weight`] of 1): with Light by on, a star's core is its weight over this.
pub const LIT_BRIGHTEST: f32 = 1.5;
/// About how many of the best lit stars on the view show as points of their own when it's far out, and how many get
/// spikes: a few thousand points read as "these", more would fill the map.
const GLINTS: f32 = 2_500.0;
const SPIKY: f32 = 250.0;
/// Spikes only ever go to the top 1.5 in a hundred.
const SPIKES_FROM: f32 = 0.985;

/// How much of what's on show has the Light by measure (0 to 1): the share of the `shown` sounds whose `light` is
/// above 0. 0 with nothing lit or nothing shown. Only the shown ones count: with a filter for loops and Light by Tempo,
/// every loop on show has a tempo, whatever share of the whole list does. Above 0 is what "Light by is on" means
/// everywhere (the card's stars, its shader and the processor's picture): with none of the shown sounds having the
/// measure, every star is drawn alike, as with Nothing.
pub fn lit_share(light: &[f32], shown: &[usize]) -> f32 {
    if light.is_empty() || shown.is_empty() {
        return 0.0;
    }
    shown.par_iter().filter(|&&at| light.get(at).is_some_and(|&l| l > 0.0)).count() as f32 / shown.len() as f32
}

/// The shown stars that have the Light by measure (`light` above 0), by their place in the list, from the least lit
/// to the most. Equally lit ones go in a steady order by where they are on the map rather than by their place in the
/// list, so when only some of a crowd of equally lit sounds can glint, they're spread over the map instead of all
/// coming from one folder, and they stay the same ones when the list changes. The stars that glint far out and the
/// ones with spikes are the last of these ([`glinting`]), on the card and on the processor alike. Empty with nothing
/// lit. Made on every core.
pub fn lit_order(stars: &[Option<StarPoint>], shown: &[usize], light: &[f32]) -> Vec<u32> {
    if light.is_empty() {
        return Vec::new();
    }
    let mut lit: Vec<(u32, u32, u32)> = shown
        .par_iter()
        .filter_map(|&at| {
            let p = (*stars.get(at)?)?;
            let l = *light.get(at)?;
            // A light above 0 sorts by its bits as by its value.
            (l > 0.0).then(|| (l.to_bits(), spread(p), u32::try_from(at).unwrap_or(u32::MAX)))
        })
        .collect();
    lit.par_sort_unstable();
    lit.into_iter().map(|(_, _, at)| at).collect()
}

/// A steady number for a star by where it is on the map, to put equally lit stars in an order that has nothing to do
/// with their place in the list.
fn spread(p: StarPoint) -> u32 {
    let h = p.x.to_bits().wrapping_mul(0x9E37_79B9) ^ p.y.to_bits().wrapping_mul(0x85EB_CA6B);
    (h ^ (h >> 15)).wrapping_mul(0x2C1B_3C6D)
}

/// Of `lit` stars with the measure on the map ([`lit_order`]), how many glint far out and how many get spikes, for
/// `look`: always the last ones, the best lit.
pub fn glinting(look: &Look, lit: u32) -> (u32, u32) {
    let top = |from: f32| ((lit as f32 * (1.0 - from)).round() as u32).min(lit);
    (top(look.glint_from), top(look.spikes_from))
}

/// How strongly the star `k` places into the ones that glint (of `of`, the least lit first) shows as a point of its
/// own when the view is painted (0 to 1). The graphics card's shader works it out the same way.
pub fn glint_at(k: u32, of: u32) -> f32 {
    smooth(0.0, 1.0, k as f32 / of.max(1) as f32)
}

/// A star's radius on the processor's picture with Light by on, in pixels: `near` close up (its size by how lit it is,
/// as [`lit_size`]); far out, a star that glints (`glint` above 0) is a point of [`MIN_LIT`] points (`min_lit`
/// pixels) or a little more, and grows to `near` as the view flies in (`crisp` 0 far out, 1 close up).
///
/// The processor draws every star as a dot at every zoom (it has no painted layer), so a star that glints is never
/// drawn smaller than `near` either, or it would be smaller than the less lit one beside it. The card paints the
/// stars that don't glint far out, so its shader lets a glint be the smaller point (see `mo-ui`'s `sounds::sky`).
pub fn glint_point(near: f32, glint: f32, crisp: f32, min_lit: f32) -> f32 {
    if glint <= 0.0 {
        return near;
    }
    let far = (min_lit * (1.0 + 0.6 * glint)).max(near);
    far + (near - far) * crisp.clamp(0.0, 1.0)
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
    /// From how lit a star shows as a point of its own far out, and from how lit it gets spikes.
    pub glint_from: f32,
    pub spikes_from: f32,
}

/// A smooth step from 0 at `a` to 1 at `b`.
fn smooth(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The look for `view` with `in_view` shown stars on it, `lit_share` of which have the Light by measure (see
/// [`lit_share`]; 0 with nothing lit).
pub fn look(view: &View, in_view: usize, lit_share: f32) -> Look {
    let n = in_view as f32;
    // Light ranks the sounds that have the measure, so the top ones are a share of those on the view.
    let lit_on_view = (n * lit_share.clamp(0.0, 1.0)).max(1.0);
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
        glint_from: (1.0 - GLINTS / lit_on_view).clamp(0.6, 0.995),
        spikes_from: (1.0 - SPIKY / lit_on_view).clamp(SPIKES_FROM, 0.9995),
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

/// Every star on the map as the graphics card reads it ([`STAR_BYTES`] each): first the ones the filters leave out, so
/// the shown ones are drawn over them; then the shown ones without the Light by measure; then the ones with it, in
/// `order` (the [`lit_order`] of the same `stars`, `shown` and `light`), the least lit first. So the best lit are
/// drawn on top and come last: far out only those few are drawn as stars, and the card is told where they start
/// ([`glint_starts`]). `colours` is each star's place in `palette` (anything else is the last colour, grey); `shown`
/// is the places the filters let through; `light` how lit each is (see [`light`]), which rides in the colour's last
/// byte (0 for a left-out star, 1 to 255 for a shown one). With none of the shown stars lit (`order` empty), every
/// shown one is drawn alike, as with Nothing. Gives the bytes and how many stars at their end are the lit ones. Made
/// on every core.
pub fn pack(
    stars: &[Option<StarPoint>],
    colours: &[i32],
    palette: &[[u8; 3]],
    shown: &[usize],
    light: &[f32],
    order: &[u32],
) -> (Vec<u8>, u32) {
    // 0 left out, 1 shown, 2 shown and in `order` (drawn last, in its order).
    let mut part = vec![0u8; stars.len()];
    for &at in shown {
        if let Some(o) = part.get_mut(at) {
            *o = 1;
        }
    }
    for &at in order {
        if let Some(o) = part.get_mut(at as usize) {
            *o = 2;
        }
    }
    let lighting = !order.is_empty();
    let grey = palette.last().copied().unwrap_or([110, 110, 110]);
    let star = |at: usize| -> Option<[u8; STAR_BYTES]> {
        let p = (*stars.get(at)?)?;
        let want = part[at] > 0;
        let c = colours.get(at).and_then(|&i| usize::try_from(i).ok()).and_then(|i| palette.get(i)).copied();
        let [r, g, b] = c.unwrap_or(grey);
        let lit = if lighting { light.get(at).copied().unwrap_or(0.0).clamp(0.0, 1.0) } else { 0.0 };
        let weight = match (want, lighting) {
            (false, _) => 0.05,
            (true, false) => ALIKE_WEIGHT,
            (true, true) => lit_weight(lit),
        } * twinkle(at);
        let flag = if want { 1 + (lit * 254.0).round() as u8 } else { 0 };
        let mut out = [0u8; STAR_BYTES];
        out[0..4].copy_from_slice(&p.x.to_le_bytes());
        out[4..8].copy_from_slice(&p.y.to_le_bytes());
        out[8..12].copy_from_slice(&[r, g, b, flag]);
        out[12..16].copy_from_slice(&weight.to_le_bytes());
        Some(out)
    };
    let mut bytes: Vec<u8> = Vec::with_capacity(stars.len() * STAR_BYTES);
    for want in [0, 1] {
        let some: Vec<[u8; STAR_BYTES]> =
            (0..stars.len()).into_par_iter().filter(|&at| part[at] == want).filter_map(star).collect();
        bytes.extend_from_slice(some.as_flattened());
    }
    let lit: Vec<[u8; STAR_BYTES]> =
        order.par_iter().filter(|&&at| part.get(at as usize) == Some(&2)).filter_map(|&at| star(at as usize)).collect();
    bytes.extend_from_slice(lit.as_flattened());
    (bytes, u32::try_from(lit.len()).unwrap_or(u32::MAX))
}

/// Where in what [`pack`] made (`count` stars, the last `lit` of them the ones in its `order`) the stars that show as
/// points of their own far out start, and the ones with spikes, for `look` ([`glinting`]): it's their rank that
/// counts, not their light byte, which can't tell the top 250 of 450,000 apart. Both are `count` (none) with nothing
/// lit.
pub fn glint_starts(look: &Look, lit: u32, count: u32) -> (u32, u32) {
    let (glints, spikes) = glinting(look, lit.min(count));
    (count - glints, count - spikes)
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
        assert_eq!(look(&v, 450_000, 0.0).painted, 1.0);
        assert_eq!(look(&v, 200, 0.0).painted, 0.0);
        let between = look(&v, 3_000, 0.0).painted;
        assert!(between > 0.0 && between < 1.0);
        // A crowded view squeezes gently, a sparse one fully.
        assert!(look(&v, 450_000, 0.0).gain < look(&v, 2_000, 0.0).gain);
        assert_eq!(look(&v, 10, 0.0).gain, 1.7);
    }

    #[test]
    fn stars_grow_as_the_view_flies_in_but_keep_their_room() {
        let mut v = view();
        let far = look(&v, 100, 0.0).radius;
        v.zoom = 20.0;
        let near = look(&v, 100, 0.0).radius;
        assert!(near > far);
        // Very crowded: they shrink to keep apart, never below 1.6 points.
        assert_eq!(look(&v, 1_000_000, 0.0).radius, 1.6);
    }

    #[test]
    fn the_left_out_stars_come_first_and_the_shown_ones_are_marked() {
        let stars = vec![Some(StarPoint { x: 0.25, y: 0.5 }), None, Some(StarPoint { x: 0.75, y: 0.1 })];
        let palette = [[200, 100, 50], [110, 110, 110]];
        let light = [1.0, 0.0, 0.0];
        let (bytes, lit) = pack(&stars, &[0, 0, 7], &palette, &[0], &light, &lit_order(&stars, &[0], &light));
        assert_eq!((bytes.len(), lit), (2 * STAR_BYTES, 1));
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
        // The two sounds added in 2022 share the middle of the top two places. The oldest is only just lit: a sound
        // with the measure is never 0, which is what a sound without it is.
        assert_eq!(light(&list, LightBy::Newest), [f32::MIN_POSITIVE, 1.0 / 3.0, 2.5 / 3.0, 2.5 / 3.0]);
        assert_eq!(lit_share(&light(&list, LightBy::Newest), &[0, 1, 2, 3]), 1.0);
        // Nothing lit, or nothing has the measure (use counts aren't in yet): every star alike.
        assert!(light(&list, LightBy::Nothing).is_empty());
        assert!(light(&list, LightBy::Used).is_empty());
        assert_eq!(LightBy::from_name("Used in songs"), Some(LightBy::Used));
    }

    #[test]
    fn of_450000_lit_stars_exactly_the_top_250_get_spikes_and_2500_glint() {
        let count = 450_000u32;
        let lk = look(&view(), count as usize, 1.0);
        let (glints, spikes) = glinting(&lk, count);
        assert!(spikes.abs_diff(250) <= 1, "{spikes}");
        assert!(glints.abs_diff(2_500) <= 2, "{glints}");
        // They're the last ones the card holds; none with nothing lit.
        assert_eq!(glint_starts(&lk, count, count), (count - glints, count - spikes));
        assert_eq!(glint_starts(&lk, 0, count), (count, count));
        // The ramp: from nothing at the first that glints to fully at the last.
        assert_eq!(glint_at(0, glints), 0.0);
        assert_eq!(glint_at(glints, glints), 1.0);
        assert!(glint_at(glints / 2, glints) > glint_at(glints / 4, glints));
    }

    #[test]
    fn far_out_only_the_best_lit_few_thousand_glint_and_a_few_hundred_get_spikes() {
        let v = view();
        // Length: every sound has the measure. 450,000 on the view: the top 2,500 show as points, 250 get spikes.
        let all = look(&v, 450_000, 1.0);
        assert!((all.glint_from - (1.0 - 2_500.0 / 450_000.0)).abs() < 1e-6);
        assert!((all.spikes_from - (1.0 - 250.0 / 450_000.0)).abs() < 1e-6);
        // Liked: 2,000 liked sounds. Their top 40 in a hundred glint; spikes for the top 1.5 in a hundred at most.
        let liked = look(&v, 450_000, 2_000.0 / 450_000.0);
        assert_eq!(liked.glint_from, 0.6);
        assert_eq!(liked.spikes_from, 0.985);
        assert_eq!(glinting(&liked, 2_000), (800, 30));
    }

    #[test]
    fn a_star_that_glints_is_a_point_far_out_and_never_smaller_than_one_less_lit() {
        // Far out a glint is at least the smallest point, a little more for the best lit, and grows to its own size
        // as the view flies in.
        assert_eq!(glint_point(1.0, 0.0, 0.0, 1.6), 1.0);
        assert_eq!(glint_point(1.0, 0.5, 0.0, 1.6), 1.6 * 1.3);
        assert_eq!(glint_point(1.0, 1.0, 1.0, 1.6), 1.0);
        assert!(glint_point(1.0, 1.0, 0.5, 1.6) > 1.0 && glint_point(1.0, 1.0, 0.5, 1.6) < 1.6 * 1.6);
        // A big one stays its own size: never smaller than the one just under it that doesn't glint.
        assert_eq!(glint_point(2.35, 0.01, 0.0, 1.6), 2.35);
        // Twice the scale, twice the point.
        assert_eq!(glint_point(1.0, 0.5, 0.0, 3.2), 3.2 * 1.3);
    }

    #[test]
    fn with_light_by_on_the_unlit_stars_are_dimmer_than_all_alike_ones_and_the_lit_brighter() {
        assert!(lit_weight(0.0) < ALIKE_WEIGHT);
        assert!(lit_weight(1.0) >= 3.0 * ALIKE_WEIGHT);
        let stars = vec![Some(StarPoint { x: 0.5, y: 0.5 })];
        let weight = |light: &[f32]| {
            let (b, _) = pack(&stars, &[0], &[[1, 2, 3]], &[0], light, &lit_order(&stars, &[0], light));
            f32::from_le_bytes(b[12..16].try_into().unwrap())
        };
        let alike = weight(&[]);
        assert!(weight(&[1.0]) > alike);
        // A star without the measure is dimmer than an all-alike one only when some star on show has it: with none of
        // them lit, they're all drawn alike, as with Nothing.
        assert_eq!(weight(&[0.0]), alike);
        let two = vec![Some(StarPoint { x: 0.2, y: 0.5 }), Some(StarPoint { x: 0.8, y: 0.5 })];
        let light = [0.0, 1.0];
        let (b, lit) = pack(&two, &[0, 0], &[[1, 2, 3]], &[0, 1], &light, &lit_order(&two, &[0, 1], &light));
        assert_eq!(lit, 1);
        assert!(f32::from_le_bytes(b[12..16].try_into().unwrap()) < alike);
        assert_eq!(lit_weight(1.0), LIT_BRIGHTEST);
        assert_eq!(lit_share(&[0.0, 0.5, 1.0, 0.0], &[0, 1, 2, 3]), 0.5);
        // Only what's on show counts.
        assert_eq!(lit_share(&[0.0, 0.5, 1.0, 0.0], &[1, 2]), 1.0);
        assert_eq!(lit_share(&[], &[0]), 0.0);
        assert_eq!(lit_share(&[1.0], &[]), 0.0);
    }

    #[test]
    fn the_shown_stars_go_from_least_lit_to_most_so_the_best_lit_come_last() {
        let stars: Vec<Option<StarPoint>> = (0..6).map(|i| Some(StarPoint { x: i as f32 / 6.0, y: 0.5 })).collect();
        let light = [1.0, 0.0, 0.5, 0.9, 0.2, 0.3];
        // The fourth is left out, the second has no measure.
        let shown = [0, 1, 2, 4, 5];
        let order = lit_order(&stars, &shown, &light);
        assert_eq!(order, [4, 5, 2, 0]);
        let (bytes, lit) = pack(&stars, &[0; 6], &[[9, 9, 9]], &shown, &light, &order);
        assert_eq!(lit, 4);
        let flags: Vec<u8> = bytes.as_chunks::<STAR_BYTES>().0.iter().map(|s| s[11]).collect();
        assert_eq!(flags, [0, 1, 1 + 51, 1 + 76, 1 + 127, 255]);
        // Four of the five shown have the measure.
        assert_eq!(lit_share(&light, &shown), 0.8);
        let mut lk = look(&view(), 5, 0.8);
        (lk.glint_from, lk.spikes_from) = (0.5, 0.75);
        // The top half of the four lit glint (2), the top quarter (1) gets spikes: the last stars on the card.
        assert_eq!(glinting(&lk, lit), (2, 1));
        assert_eq!(glint_starts(&lk, lit, 6), (4, 5));
    }

    #[test]
    fn equally_lit_stars_go_in_an_order_by_where_they_are_not_by_their_place_in_the_list() {
        // A crowd of equally lit stars in a row (as one folder of sounds with the same tempo would be in the list):
        // the order among them doesn't follow the list, and it's the same whatever the list's order.
        let stars: Vec<Option<StarPoint>> =
            (0..64).map(|i| Some(StarPoint { x: (i % 8) as f32 / 8.0, y: (i / 8) as f32 / 8.0 })).collect();
        let shown: Vec<usize> = (0..64).collect();
        let order = lit_order(&stars, &shown, &[0.5; 64]);
        assert_eq!(order.len(), 64);
        assert_ne!(order, (0..64).collect::<Vec<u32>>());
        let last_eight: Vec<u32> = order[56..].to_vec();
        assert!(last_eight.iter().any(|&at| at < 32) && last_eight.iter().any(|&at| at >= 32), "{last_eight:?}");
        // The same stars the other way round in the list: the same ones come last.
        let reversed: Vec<Option<StarPoint>> = stars.iter().rev().copied().collect();
        let back: Vec<u32> = lit_order(&reversed, &shown, &[0.5; 64]).iter().map(|&at| 63 - at).collect();
        assert_eq!(back, order);
        // A brighter one still comes after them all.
        let mut light = [0.5; 64];
        light[3] = 0.6;
        assert_eq!(lit_order(&stars, &shown, &light).last(), Some(&3));
        assert!(lit_order(&stars, &shown, &[]).is_empty());
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
