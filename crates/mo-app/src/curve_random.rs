//! Randomise (Solomon, 14 Nov 2026): a new curve made from parts of the curves shown in the list. Each part is cut
//! from one of them on its own rhythm (a whole number of the part's length from its start) and lands as a clip named
//! after the curve it came from. "How wild" adds what the highlight's handles do: another speed, backwards, upside
//! down, a stutter, a fade. Parts come back (a phrase is used again), a pinned clip stays as it is, and with a
//! highlight only that part is rolled again. The same seed rolls the same curve.
//!
//! It's built from the editor's own edits ([`crate::curve_edit::apply`]), so a rolled curve is nothing the handles
//! couldn't have made.

use crate::curve_edit::{Edit, MAX_BEATS, Piece, SHORTEST, Shape, Span, apply};
use crate::curve_picture::BAR;
use mo_formats::bitwig::curve::CurvePoint;

/// A curve from the list, to cut parts from.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub id: String,
    pub name: String,
    /// Its points, 0 to 1 across.
    pub points: Vec<CurvePoint>,
    pub length_beats: f64,
}

/// The lengths a roll can be, in bars.
pub const BARS: [u32; 4] = [1, 2, 4, 8];

/// How to roll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// The new curve's length in bars (one of [`BARS`]); not used when only a highlight is rolled.
    pub bars: u32,
    /// 0 ("Spliced": parts as they are) to 1 ("Wild": most parts changed).
    pub wild: f64,
    /// Parts of different lengths, dotted ones too; else every part is the same length.
    pub mixed: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { bars: 2, wild: 0.3, mixed: false }
    }
}

/// The slider's place in words.
pub fn wild_name(wild: f64) -> &'static str {
    match wild {
        w if w < 0.125 => "Spliced",
        w if w < 0.45 => "A little",
        w if w < 0.75 => "Halfway",
        _ => "Wild",
    }
}

/// The words under RANDOMISE: what it makes and from how many, or why it can't.
pub fn says(shown: usize) -> String {
    if shown < 2 {
        "Show at least 2 curves to randomise.".to_owned()
    } else {
        format!("From the {shown} curves shown.")
    }
}

/// What the card says after a roll cut from `used` of the `shown` curves (a long list is sampled).
pub fn rolled_words(used: usize, shown: usize) -> String {
    let from =
        if used < shown { format!("{used} of the {shown} curves shown") } else { format!("the {shown} curves shown") };
    format!("Rolled from {from}. Undo brings the old one back; Save as new curve keeps this one.")
}

/// What the card says when a roll made nothing.
pub const NOTHING_ROLLED: &str =
    "Nothing was rolled: it takes two curves with a shape, and room that isn't in a pinned clip.";

/// SplitMix64: a small, fast random sequence.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0 up to (not including) 1.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// 0 up to (not including) `n`; 0 when `n` is 0.
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next()) * n as u128) >> 64) as usize
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }
}

/// What's done to one part: which curve it's cut from, how far in (in parts of its own length), at what speed, and
/// what "How wild" added.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Recipe {
    source: usize,
    /// Which of the source's windows of this length, counted from its start.
    window: usize,
    /// The source's time played in the part's: 1 as it is, 2 twice as fast, 0.5 half.
    speed: f64,
    reverse: bool,
    flip: bool,
    /// The part's first half or quarter repeated to fill it: 2 or 4; 0 for none.
    stutter: u32,
    /// Fades in (below 0) or out (above 0) by this much; 0 for none.
    fade: f64,
}

/// `edit` done, or the shape as it was when it changes nothing or can't be done.
fn with(shape: Shape, highlight: Span, edit: &Edit) -> Shape {
    match apply(&shape, Some(highlight), edit) {
        Some((after, _)) => after,
        None => shape,
    }
}

/// How long a source is for cutting: its own length, a bar when that isn't known, never past what the editor holds.
fn usable(length_beats: f64) -> f64 {
    if length_beats.is_finite() && length_beats > 0.0 { length_beats.clamp(SHORTEST, MAX_BEATS) } else { BAR }
}

/// `length` beats of a curve from `start`, as a curve of its own: its points 0 to 1 across, and how long it is. A
/// curve shorter than `length` goes round again to fill it (never stretched). None when that much can't be cut (a
/// sliver shorter than the editor's shortest).
fn window(points: &[CurvePoint], whole: f64, start: f64, length: f64) -> Option<(Vec<CurvePoint>, f64)> {
    let mut s = Shape::from_curve(points, whole);
    let whole = s.length();
    if length > whole + 1e-9 {
        // Made as long as asked, then its own length repeated across.
        s = apply(&s, None, &Edit::SetLength(length))?.0;
        let all = Span::new(0.0, s.length());
        s = apply(&s, Some(all), &Edit::Repeat { start: 0.0, end: whole })?.0;
    } else {
        let end = (start + length).min(whole);
        if end < whole - 1e-9 {
            s = apply(&s, None, &Edit::SetLength(end))?.0;
        }
        if start > 1e-9 {
            s = apply(&s, Some(Span::new(0.0, start)), &Edit::RemoveTime)?.0;
        }
    }
    ((s.length() - length).abs() < 1e-6).then(|| (s.curve_points(), s.length()))
}

/// The lengths of the parts that fill `room` beats. Not mixed: all `unit` long (the last one what's left). Mixed:
/// each drawn afresh, dotted ones too, none longer than `most`.
fn part_lengths(rng: &mut Rng, room: f64, unit: f64, mixed: bool, most: f64) -> Vec<f64> {
    const MIXED: [f64; 6] = [0.5, 1.0, 1.5, 2.0, 3.0, 4.0];
    let mut out: Vec<f64> = Vec::new();
    let mut left = room;
    while left > SHORTEST {
        let len = if mixed {
            let fits: Vec<f64> = MIXED.iter().copied().filter(|l| *l <= left.min(most) + 1e-9).collect();
            if fits.is_empty() { left } else { fits.get(rng.below(fits.len())).copied().unwrap_or(left) }
        } else {
            unit.min(left)
        };
        // A sliver left over joins the part before it.
        if left - len <= SHORTEST {
            out.push(left);
            break;
        }
        out.push(len);
        left -= len;
    }
    out
}

/// A new recipe for a part `len` beats long, not cut from the curve `not` (the part before it came from that one: a
/// roll mixes its curves).
fn recipe(rng: &mut Rng, sources: &[Source], len: f64, wild: f64, not: Option<usize>) -> Recipe {
    let source = match not {
        Some(not) if sources.len() > 1 => (not + 1 + rng.below(sources.len() - 1)) % sources.len(),
        _ => rng.below(sources.len()),
    };
    let whole = sources.get(source).map_or(BAR, |s| usable(s.length_beats));
    let speed = if rng.chance(0.5 * wild) { if rng.chance(0.5) { 2.0 } else { 0.5 } } else { 1.0 };
    // Cut on the source's own rhythm: a whole number of the cut's length from its start.
    let cut = len * speed;
    let windows = ((whole / cut) + 1e-9).floor().max(1.0) as usize;
    Recipe {
        source,
        window: rng.below(windows),
        speed,
        reverse: rng.chance(0.35 * wild),
        flip: rng.chance(0.25 * wild),
        stutter: if rng.chance(0.35 * wild) && len >= 0.5 { if rng.chance(0.5) { 2 } else { 4 } } else { 0 },
        fade: if rng.chance(0.3 * wild) {
            let by = 0.5 + 0.5 * rng.unit();
            if rng.chance(0.5) { by } else { -by }
        } else {
            0.0
        },
    }
}

/// Puts a part made by `r` over `span` of `shape`, as a clip named after its source.
fn put(shape: Shape, span: Span, r: Recipe, sources: &[Source]) -> Shape {
    let Some(src) = sources.get(r.source) else { return shape };
    let whole = usable(src.length_beats);
    // A curve shorter than the cut goes round again from its start.
    let cut = span.len() * r.speed;
    let start = if cut > whole { 0.0 } else { (r.window as f64 * cut).min(whole - cut) };
    let Some((points, length_beats)) = window(&src.points, whole, start, cut) else { return shape };
    let piece = Piece { points, length_beats, source: Some(src.id.clone()), name: src.name.clone() };
    let mut shape = with(shape, span, &Edit::Swap { piece, keep_length: false });
    if r.reverse {
        shape = with(shape, span, &Edit::Reverse);
    }
    if r.flip {
        shape = with(shape, span, &Edit::Flip);
    }
    if r.stutter > 0 {
        let kept = span.len() / f64::from(r.stutter);
        shape = with(shape, span, &Edit::Repeat { start: span.start, end: span.start + kept });
    }
    if r.fade != 0.0 {
        let kept = 1.0 - r.fade.abs();
        let (left, right) = if r.fade < 0.0 { (kept, 1.0) } else { (1.0, kept) };
        shape = with(shape, span, &Edit::Tilt { left, right, from_top: false });
    }
    shape
}

/// The stretches of `room` no pinned clip of `shape` covers, in order.
fn free(shape: &Shape, room: Span) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = room.start;
    for c in shape.clips().iter().filter(|c| c.pinned && c.span.end > room.start && c.span.start < room.end) {
        if c.span.start > at + SHORTEST {
            out.push(Span::new(at, c.span.start));
        }
        at = at.max(c.span.end);
    }
    if room.end > at + SHORTEST {
        out.push(Span::new(at, room.end));
    }
    out
}

/// Rolls a curve from `sources` (at least two). With `only`, that part of `now` is rolled again and the rest stays;
/// without, the whole curve is new, `settings.bars` long, and `now`'s pinned clips that fit in it stay where they
/// are. None when there's nothing to roll from, or nowhere to put it.
pub fn roll(now: &Shape, only: Option<Span>, sources: &[Source], settings: Settings, seed: u64) -> Option<Shape> {
    if sources.len() < 2 {
        return None;
    }
    let mut rng = Rng(seed);
    let wild = if settings.wild.is_finite() { settings.wild.clamp(0.0, 1.0) } else { 0.0 };
    let (mut shape, room) = match only {
        Some(span) => (now.clone(), span),
        None => {
            let total = f64::from(settings.bars.clamp(1, 64)) * BAR;
            let level = [CurvePoint { x: 0.0, y: 0.5, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.5, bend: 0.0 }];
            let mut shape = Shape::from_curve(&level, total);
            // The pinned clips that fit stay: their parts are carried over as they are.
            let kept = now.curve_points();
            for c in now.clips().iter().filter(|c| c.pinned && c.span.end <= total + 1e-9) {
                let Some((points, length_beats)) = window(&kept, now.length(), c.span.start, c.span.len()) else {
                    continue;
                };
                let piece = Piece { points, length_beats, source: c.source.clone(), name: c.name.clone() };
                shape = with(shape, c.span, &Edit::Swap { piece, keep_length: false });
                if let Some(clip) = shape.clips().iter().position(|n| n.span.same(c.span)) {
                    shape = with(shape, c.span, &Edit::Pin { clip, pinned: true });
                }
            }
            (shape, Span::new(0.0, total))
        }
    };
    let gaps = free(&shape, room);
    if gaps.is_empty() {
        return None;
    }
    // Every part the same length unless mixed: a bar, a half or a beat, never longer than half the room.
    let units: Vec<f64> =
        [4.0, 2.0, 2.0, 1.0, 1.0, 0.5].into_iter().filter(|u| *u <= room.len() / 2.0 + 1e-9).collect();
    let unit = units.get(rng.below(units.len())).copied().unwrap_or(room.len());
    let mut used: Vec<(f64, Recipe)> = Vec::new();
    for gap in gaps {
        let mut at = gap.start;
        for len in part_lengths(&mut rng, gap.len(), unit, settings.mixed, (room.len() / 2.0).max(0.5)) {
            // A phrase comes back: a part of this length made before is used again.
            let again: Vec<Recipe> = used.iter().filter(|(l, _)| (l - len).abs() < 1e-9).map(|(_, r)| *r).collect();
            let r = match again.get(rng.below(again.len())) {
                Some(r) if rng.chance(0.35) => *r,
                _ => recipe(&mut rng, sources, len, wild, used.last().map(|(_, last)| last.source)),
            };
            used.push((len, r));
            shape = put(shape, Span::new(at, at + len), r, sources);
            at += len;
        }
    }
    Some(shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> CurvePoint {
        CurvePoint { x, y, bend: 0.0 }
    }

    fn sources() -> Vec<Source> {
        vec![
            Source {
                id: "rise".into(),
                name: "Rise".into(),
                points: vec![p(0.0, 0.0), p(1.0, 1.0)],
                length_beats: 4.0,
            },
            Source {
                id: "gate".into(),
                name: "Gate".into(),
                points: vec![p(0.0, 1.0), p(0.5, 1.0), p(0.5, 0.0), p(1.0, 0.0)],
                length_beats: 1.0,
            },
            Source {
                id: "fall".into(),
                name: "Fall".into(),
                points: vec![p(0.0, 0.9), p(1.0, 0.1)],
                length_beats: 8.0,
            },
        ]
    }

    fn level() -> Shape {
        Shape::from_curve(&[p(0.0, 0.5), p(1.0, 0.5)], 4.0)
    }

    #[test]
    fn a_roll_is_the_length_asked_for_and_all_clips_end_to_end() {
        for seed in 1..40 {
            for bars in BARS {
                let s = Settings { bars, wild: 0.5, mixed: seed % 2 == 0 };
                let rolled = roll(&level(), None, &sources(), s, seed).unwrap();
                assert!((rolled.length() - f64::from(bars) * 4.0).abs() < 1e-9);
                let mut at = 0.0;
                for c in rolled.clips() {
                    assert!((c.span.start - at).abs() < 1e-6, "seed {seed}: a gap at {at}");
                    assert!(sources().iter().any(|s| Some(&s.id) == c.source.as_ref() && s.name == c.name));
                    at = c.span.end;
                }
                assert!((at - rolled.length()).abs() < 1e-6, "seed {seed}: clips end at {at}");
                assert!(rolled.points().iter().all(|p| (0.0..=1.0).contains(&p.value) && p.beat <= rolled.length()));
            }
        }
    }

    #[test]
    fn the_same_seed_rolls_the_same_curve_and_another_seed_another() {
        let s = Settings { bars: 4, wild: 0.6, mixed: true };
        let a = roll(&level(), None, &sources(), s, 7).unwrap();
        assert_eq!(a, roll(&level(), None, &sources(), s, 7).unwrap());
        assert!((8..20).any(|seed| roll(&level(), None, &sources(), s, seed).unwrap() != a));
    }

    #[test]
    fn spliced_parts_are_cut_from_the_sources_as_they_are() {
        // With nothing wild and only rising curves to cut from, no part ever falls but at a seam.
        let rising = vec![
            Source { id: "a".into(), name: "A".into(), points: vec![p(0.0, 0.0), p(1.0, 1.0)], length_beats: 4.0 },
            Source { id: "b".into(), name: "B".into(), points: vec![p(0.0, 0.2), p(1.0, 0.6)], length_beats: 2.0 },
        ];
        let s = Settings { bars: 2, wild: 0.0, mixed: false };
        for seed in 1..20 {
            let rolled = roll(&level(), None, &rising, s, seed).unwrap();
            for c in rolled.clips() {
                let (from, to) = (rolled.value_at(c.span.start + 1e-6), rolled.value_before(c.span.end));
                assert!(to >= from - 1e-6, "seed {seed}: a part falls from {from} to {to}");
            }
        }
    }

    #[test]
    fn a_pinned_clip_stays_and_a_highlight_rolls_only_its_part() {
        let s = Settings { bars: 2, wild: 1.0, mixed: true };
        let first = roll(&level(), None, &sources(), s, 3).unwrap();
        let keep = first.clips()[0].clone();
        let (pinned, _) = apply(&first, None, &Edit::Pin { clip: 0, pinned: true }).unwrap();
        let again = roll(&pinned, None, &sources(), s, 99).unwrap();
        let kept = again.clips().iter().find(|c| c.pinned).expect("the pinned clip is gone");
        assert_eq!((kept.span, &kept.name), (keep.span, &keep.name));
        for i in 0..=20 {
            let beat = keep.span.start + keep.span.len() * f64::from(i) / 20.0 * 0.999;
            assert!((again.value_at(beat) - first.value_at(beat)).abs() < 1e-6, "the pinned part changed at {beat}");
        }
        // Only the highlight: what's outside is as it was, and the curve is no longer or shorter.
        let lit = Span::new(4.0, 6.0);
        let part = roll(&first, Some(lit), &sources(), s, 5).unwrap();
        assert!((part.length() - first.length()).abs() < 1e-9);
        for beat in [0.1, 1.0, 2.5, 3.9, 6.1, 7.0, 7.9] {
            assert!((part.value_at(beat) - first.value_at(beat)).abs() < 1e-6, "changed outside at {beat}");
        }
        assert!(part.clips().iter().any(|c| c.span.start >= 4.0 - 1e-9 && c.span.end <= 6.0 + 1e-9));
    }

    #[test]
    fn it_needs_two_curves_and_somewhere_to_put_them() {
        let s = Settings::default();
        assert_eq!(roll(&level(), None, &sources()[..1], s, 1), None);
        assert_eq!(says(1), "Show at least 2 curves to randomise.");
        assert_eq!(says(19), "From the 19 curves shown.");
        assert!(rolled_words(4, 4).starts_with("Rolled from the 4 curves shown."));
        assert!(rolled_words(64, 500).starts_with("Rolled from 64 of the 500 curves shown."));
        assert_eq!(
            (wild_name(0.0), wild_name(0.3), wild_name(0.6), wild_name(1.0)),
            ("Spliced", "A little", "Halfway", "Wild")
        );
        // A curve that's all one pinned clip has no room left.
        let (clip, _) = apply(&level(), Some(Span::new(0.0, 4.0)), &Edit::Remember { name: "All".into() }).unwrap();
        let (pinned, _) = apply(&clip, None, &Edit::Pin { clip: 0, pinned: true }).unwrap();
        assert_eq!(roll(&pinned, None, &sources(), Settings { bars: 1, ..s }, 1), None);
    }

    #[test]
    fn a_short_curve_goes_round_again_and_is_never_stretched() {
        // A one-beat ramp cut for four beats: four ramps, each a beat long.
        let ramp = [p(0.0, 0.0), p(1.0, 1.0)];
        let (points, length) = window(&ramp, 1.0, 0.0, 4.0).expect("cut");
        assert!((length - 4.0).abs() < 1e-9);
        let s = Shape::from_curve(&points, length);
        for beat in 0..4 {
            assert!(s.value_at(f64::from(beat) + 0.5) > 0.4 && s.value_at(f64::from(beat) + 0.5) < 0.6, "beat {beat}");
        }
    }

    #[test]
    fn mixed_parts_are_never_longer_than_half_the_room() {
        for seed in 0..200 {
            let mut rng = Rng(seed);
            let parts = part_lengths(&mut rng, 4.0, 1.0, true, 2.0);
            assert!(parts.len() >= 2, "seed {seed}: {parts:?}");
            assert!((parts.iter().sum::<f64>() - 4.0).abs() < 1e-9);
        }
    }

    #[test]
    fn a_new_part_is_not_cut_from_the_curve_before_it() {
        let sources = [
            Source { id: "a".into(), name: "A".into(), points: vec![p(0.0, 0.0), p(1.0, 1.0)], length_beats: 8.0 },
            Source { id: "b".into(), name: "B".into(), points: vec![p(0.0, 1.0), p(1.0, 0.0)], length_beats: 2.5 },
        ];
        for seed in 0..200 {
            let mut rng = Rng(seed);
            let r = recipe(&mut rng, &sources, 1.0, 1.0, Some(0));
            assert_eq!(r.source, 1, "seed {seed}");
            // Its window is one of that curve's own.
            assert!(r.window == 0 || (r.window + 1) as f64 * r.speed <= 2.5 + 1e-9, "seed {seed}");
        }
    }
}
