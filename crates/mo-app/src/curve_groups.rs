//! What kind of curve a curve is, for the curves list's filters (Solomon, 13 Nov: "are there categories of curves
//! you can think of to filter so I'm not just going through a list", "what about triplets or 16th notes or 8th notes"):
//! its shape (rises, pumps, gates...), its rhythm (how often it repeats, as a note length) and its feel (smooth or
//! sharp). Worked out from its points alone, once, when the list is read.
//!
//! How: the curve is drawn at 16 places per 1/32 note (512 to 65536) ([`value_at`], with the same bends the page draws), its height
//! stretched to 0 to 1. Its rhythm is the shortest note length it repeats at, at least twice, in its length in beats
//! (a plugin's shape is one bar); one that doesn't repeat is named by its whole length. Its shape is read from one
//! repeat: mostly flat at two heights is a gate, at three or more steps; else the moves up and down of at least a
//! fifth of its height are counted: one move up is a rise, one down a fall; a fast drop and a slower climb (a
//! sidechain duck) is a pump, a fast climb and a slower drop a fall; up and down at much the same speed is up and
//! down; more moves than that are other. Smooth is a curve whose sloped lines are mostly bent.

use mo_formats::bitwig::curve::CurvePoint;

/// What kind of curve it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Groups {
    pub shape: Shape,
    /// Its rhythm: one note length, or Mixed when parts of it go at different ones.
    pub rhythm: Rhythm,
    /// Every note length it goes at: the one, or each a Mixed curve uses (it's found under each).
    pub rhythms: Vec<Rhythm>,
    pub feel: Feel,
}

impl Groups {
    /// Whether it goes at `r` (a Mixed curve at each of its note lengths, and under Mixed).
    pub fn goes_at(&self, r: Rhythm) -> bool {
        self.rhythm == r || self.rhythms.contains(&r)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    Rise,
    Fall,
    Pump,
    UpDown,
    Gate,
    Steps,
    #[default]
    Other,
}

impl Shape {
    pub const ALL: [Shape; 7] =
        [Shape::Rise, Shape::Fall, Shape::Pump, Shape::UpDown, Shape::Gate, Shape::Steps, Shape::Other];

    pub fn label(self) -> &'static str {
        match self {
            Shape::Rise => "Rise",
            Shape::Fall => "Fall",
            Shape::Pump => "Pump",
            Shape::UpDown => "Up and down",
            Shape::Gate => "Gate",
            Shape::Steps => "Steps",
            Shape::Other => "Other",
        }
    }

    pub fn from_label(l: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|s| s.label() == l)
    }
}

/// How often it repeats, as a note length (in beats of a 4/4 bar).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rhythm {
    Bar,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
    QuarterTriplet,
    EighthTriplet,
    SixteenthTriplet,
    DottedEighth,
    /// Longer than a bar, once.
    Longer,
    /// Parts of it go at different note lengths (straight and triplets in one clip).
    Mixed,
    /// No note length: it doesn't repeat, and its length isn't one either.
    #[default]
    Free,
}

impl Rhythm {
    pub const ALL: [Rhythm; 13] = [
        Rhythm::Bar,
        Rhythm::Half,
        Rhythm::Quarter,
        Rhythm::Eighth,
        Rhythm::Sixteenth,
        Rhythm::ThirtySecond,
        Rhythm::QuarterTriplet,
        Rhythm::EighthTriplet,
        Rhythm::SixteenthTriplet,
        Rhythm::DottedEighth,
        Rhythm::Mixed,
        Rhythm::Longer,
        Rhythm::Free,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Rhythm::Bar => "1 bar",
            Rhythm::Half => "1/2",
            Rhythm::Quarter => "1/4",
            Rhythm::Eighth => "1/8",
            Rhythm::Sixteenth => "1/16",
            Rhythm::ThirtySecond => "1/32",
            Rhythm::QuarterTriplet => "1/4 triplet",
            Rhythm::EighthTriplet => "1/8 triplet",
            Rhythm::SixteenthTriplet => "1/16 triplet",
            Rhythm::DottedEighth => "Dotted 1/8",
            Rhythm::Mixed => "Mixed",
            Rhythm::Longer => "Longer",
            Rhythm::Free => "Free",
        }
    }

    pub fn from_label(l: &str) -> Option<Rhythm> {
        Rhythm::ALL.into_iter().find(|r| r.label() == l)
    }

    /// Its length in beats (none for Longer and Free).
    fn beats(self) -> Option<f64> {
        Some(match self {
            Rhythm::Bar => 4.0,
            Rhythm::Half => 2.0,
            Rhythm::Quarter => 1.0,
            Rhythm::Eighth => 0.5,
            Rhythm::Sixteenth => 0.25,
            Rhythm::ThirtySecond => 0.125,
            Rhythm::QuarterTriplet => 2.0 / 3.0,
            Rhythm::EighthTriplet => 1.0 / 3.0,
            Rhythm::SixteenthTriplet => 1.0 / 6.0,
            Rhythm::DottedEighth => 0.75,
            Rhythm::Mixed | Rhythm::Longer | Rhythm::Free => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Feel {
    Smooth,
    #[default]
    Sharp,
}

impl Feel {
    pub const ALL: [Feel; 2] = [Feel::Smooth, Feel::Sharp];

    pub fn label(self) -> &'static str {
        match self {
            Feel::Smooth => "Smooth",
            Feel::Sharp => "Sharp",
        }
    }

    pub fn from_label(l: &str) -> Option<Feel> {
        Feel::ALL.into_iter().find(|f| f.label() == l)
    }
}

/// How many places a curve is drawn at for each 1/32 note, and at least and at most in all.
const SAMPLES_PER_32ND: f64 = 16.0;
const FEWEST_SAMPLES: usize = 512;
/// Enough for 1/32 notes (8 samples each, the fewest a repeat is looked for at) in a clip 128 bars long.
const MOST_SAMPLES: usize = 65536;
/// How near two drawings must be (on average, of the curve's height) to count as the same: a repeat.
const SAME: f64 = 0.05;
/// The smallest move up or down counted, of the curve's height.
const MOVE: f64 = 0.2;
/// The stretch a curve that doesn't repeat as a whole is looked at in, to find the note lengths its parts go at:
/// half a bar.
const PART_BEATS: f64 = 2.0;

/// What kind of curve `points` (across 0 to 1) make, `length_beats` long. `loops`: it repeats as an LFO does (a
/// plugin's shape), so its end runs on into its start (a rise that ends high drops back to start again).
pub fn of(points: &[CurvePoint], length_beats: f64, loops: bool) -> Groups {
    let feel = feel(points);
    if !length_beats.is_finite() || length_beats <= 0.0 {
        return Groups { feel, ..Groups::default() };
    }
    let n = ((length_beats / 0.125 * SAMPLES_PER_32ND) as usize).clamp(FEWEST_SAMPLES, MOST_SAMPLES);
    let raw = draw(points, n);
    let (low, high) = raw.iter().fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
    let span = high - low;
    if span.is_nan() || span <= 1e-6 {
        return Groups { feel, ..Groups::default() };
    }
    let mut drawn: Vec<f64> = raw.iter().map(|v| (v - low) / span).collect();
    if let Some((rhythm, samples)) = repeat(&drawn, length_beats) {
        let mut one = drawn[..=samples].to_vec();
        one.push(one[0]);
        let shape = shape(&one, true, feel == Feel::Smooth);
        return Groups { shape, rhythm, rhythms: vec![rhythm], feel };
    }
    // Not one repeat all through: the note lengths its half bars go at, each half bar on its own.
    let parts = (length_beats / PART_BEATS).floor() as usize;
    let mut rhythms: Vec<Rhythm> = Vec::new();
    let mut shapes: Vec<Shape> = Vec::new();
    if parts >= 2 {
        let per = n as f64 / (length_beats / PART_BEATS);
        for k in 0..parts {
            let (from, to) = ((k as f64 * per).round() as usize, (((k + 1) as f64 * per).round() as usize).min(n));
            let Some(part) = drawn.get(from..=to) else { continue };
            let (lo, hi) = part.iter().fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
            if hi - lo < MOVE {
                continue;
            }
            let part: Vec<f64> = part.iter().map(|v| (v - lo) / (hi - lo)).collect();
            if let Some((r, samples)) = repeat(&part, PART_BEATS) {
                if !rhythms.contains(&r) {
                    rhythms.push(r);
                }
                shapes.push(shape(&part[..=samples], true, feel == Feel::Smooth));
            }
        }
    }
    let whole = whole_length(length_beats);
    // A loop, or a curve that ends where it began, goes round: its last move joins its first.
    let round = loops || (drawn[0] - drawn[n]).abs() < 0.05;
    if loops {
        drawn.push(drawn[0]);
    }
    let shape_of_all = shape(&drawn, round, feel == Feel::Smooth);
    match rhythms.as_slice() {
        [] => Groups { shape: shape_of_all, rhythm: whole, rhythms: vec![whole], feel },
        [one] => Groups { shape: most(&shapes).unwrap_or(shape_of_all), rhythm: *one, rhythms: vec![*one], feel },
        _ => {
            rhythms.sort_by_key(|r| Rhythm::ALL.iter().position(|a| a == r));
            Groups { shape: most(&shapes).unwrap_or(shape_of_all), rhythm: Rhythm::Mixed, rhythms, feel }
        }
    }
}

/// The shape most of `shapes` are (the first of the most, for a tie).
fn most(shapes: &[Shape]) -> Option<Shape> {
    let count = |s: &Shape| shapes.iter().filter(|t| *t == s).count();
    shapes.iter().copied().max_by(|a, b| count(a).cmp(&count(b)).then(std::cmp::Ordering::Greater))
}

/// The curve drawn at `n + 1` places evenly across, walking its lines once (as [`value_at`] draws each).
fn draw(points: &[CurvePoint], n: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(n + 1);
    let mut at = 0usize; // the first point past x
    for i in 0..=n {
        let x = i as f64 / n as f64;
        while points.get(at).is_some_and(|p| p.x <= x) {
            at += 1;
        }
        // The line from the last point at or before x: as value_at's, from a slice ending just past x.
        let upto = (at + 1).min(points.len());
        out.push(value_at(points.get(at.saturating_sub(1)..upto).unwrap_or_default(), x));
    }
    out
}

/// The curve's value at `x`, drawn as the page draws it ([`crate::presets::curve_path`]): a bent line is a
/// quadratic curve whose handle leans from the line's middle toward its late corner (bend above 0) or its early one
/// (below 0), by the bend's size. A broken bend (not a number) is straight, as on the page.
pub fn value_at(points: &[CurvePoint], x: f64) -> f64 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return 0.0 };
    if x < first.x {
        return first.y;
    }
    // The last line that starts at or before x (a jump's second point wins).
    let Some(i) = points.iter().rposition(|p| p.x <= x) else { return first.y };
    let (Some(a), Some(b)) = (points.get(i), points.get(i + 1)) else { return last.y };
    let width = b.x - a.x;
    if width <= 0.0 {
        return b.y;
    }
    let t = ((x - a.x) / width).clamp(0.0, 1.0);
    a.y + (b.y - a.y) * bent(t, a.bend)
}

/// How far along a line bent by `bend` is at `t` of its width (both 0 to 1). On a line from (0, 0) to (1, 1) the
/// page's handle is at (0.5 + bend / 2, 0.5 - bend / 2); the curve's x is solved for its step, then its height read.
fn bent(t: f64, bend: f64) -> f64 {
    let bend = if bend.is_finite() { bend.clamp(-1.0, 1.0) } else { 0.0 };
    if bend.abs() < 0.01 {
        return t;
    }
    let (cx, cy) = (0.5 + bend / 2.0, 0.5 - bend / 2.0);
    // x(s) = 2 s (1 - s) cx + s^2, so (1 - 2 cx) s^2 + 2 cx s - t = 0.
    let a = 1.0 - 2.0 * cx;
    let s = if a.abs() < 1e-9 {
        t / (2.0 * cx)
    } else {
        (-2.0 * cx + (4.0 * cx * cx + 4.0 * a * t).max(0.0).sqrt()) / (2.0 * a)
    }
    .clamp(0.0, 1.0);
    2.0 * s * (1.0 - s) * cy + s * s
}

/// The shortest note length the drawing (`length_beats` long) repeats at, at least twice, and how many samples one
/// repeat is. A repeat must move (half the drawing's height at least), so a slow ramp isn't taken for a fast one.
fn repeat(drawn: &[f64], length_beats: f64) -> Option<(Rhythm, usize)> {
    let n = drawn.len() - 1;
    let mut candidates: Vec<(Rhythm, f64)> = Rhythm::ALL.iter().filter_map(|r| Some((*r, r.beats()?))).collect();
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (r, beats) in candidates {
        let times = length_beats / beats;
        if times < 1.5 || (times - times.round()).abs() > 0.02 {
            continue;
        }
        let shift = (beats / length_beats * n as f64).round() as usize;
        if shift < 8 || shift >= n {
            continue;
        }
        // Every repeat must move, not only the first (a duck with a blip at its start isn't a fast rhythm).
        let moves = drawn.chunks(shift).filter(|c| c.len() * 2 > shift).all(|c| {
            let (lo, hi) = c.iter().fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
            hi - lo >= 0.5
        });
        if !moves {
            continue;
        }
        let differs: f64 =
            (0..=n - shift).map(|i| (drawn[i] - drawn[i + shift]).abs()).sum::<f64>() / (n - shift + 1) as f64;
        if differs < SAME {
            return Some((r, shift));
        }
    }
    None
}

/// The name of a curve's whole length, when it doesn't repeat: a note length, Longer than a bar, or Free.
fn whole_length(length_beats: f64) -> Rhythm {
    let named = Rhythm::ALL.into_iter().find(|r| r.beats().is_some_and(|b| (b - length_beats).abs() < 0.01));
    match named {
        Some(r) => r,
        None if length_beats > 4.0 => Rhythm::Longer,
        None => Rhythm::Free,
    }
}

/// One repeat's shape. `repeats`: the drawing goes on into the next repeat, so a move at its end joins one at its
/// start. `smooth`: its lines are mostly bent (a straight ramp with a jump back is a saw, a bent one a pump).
fn shape(one: &[f64], repeats: bool, smooth: bool) -> Shape {
    let n = one.len();
    if n < 3 {
        return Shape::Other;
    }
    // Flat stretches: where the value holds (to half a hundredth) for at least 1/40 of the repeat (a 1/32 step).
    let mut flat_levels: Vec<f64> = Vec::new();
    let mut flat = 0usize;
    let (mut run, mut run_level) = (1usize, one[0]);
    // Each value, then a last None that ends the last stretch.
    for v in one.iter().skip(1).map(Some).chain([None]) {
        if let Some(v) = v
            && (v - run_level).abs() < 0.005
        {
            run += 1;
            continue;
        }
        if run * 40 >= n {
            flat += run;
            if !flat_levels.iter().any(|l| (l - run_level).abs() < 0.05) {
                flat_levels.push(run_level);
            }
        }
        if let Some(v) = v {
            (run, run_level) = (1, *v);
        }
    }
    if flat * 10 >= n * 7 && flat_levels.len() >= 2 {
        return if flat_levels.len() == 2 { Shape::Gate } else { Shape::Steps };
    }
    // The moves of at least MOVE, each with how long it took (as a share of the repeat): from the last time it was at
    // the level it left to the first time it reached the one it got to (a hold in between isn't moving).
    let mut moves: Vec<(bool, f64)> = Vec::new();
    let (mut left_at, level) = (0usize, one[0]); // the last sample at the level the first move starts from
    let (mut extreme, mut first_at, mut last_at) = (one[0], 0usize, 0usize);
    let mut up: Option<bool> = None;
    for (i, &v) in one.iter().enumerate() {
        let beyond = match up {
            None => false,
            Some(true) => v > extreme + 0.005,
            Some(false) => v < extreme - 0.005,
        };
        if up.is_none() {
            if (v - level).abs() < 0.005 {
                left_at = i;
            } else if (v - level).abs() >= MOVE {
                up = Some(v > level);
                (extreme, first_at, last_at) = (v, i, i);
            }
        } else if beyond {
            (extreme, first_at, last_at) = (v, i, i);
        } else if (v - extreme).abs() < 0.005 {
            last_at = i;
        } else if (v - extreme).abs() >= MOVE {
            let going_up = up.unwrap_or(true);
            moves.push((going_up, first_at.saturating_sub(left_at) as f64 / n as f64));
            left_at = last_at;
            up = Some(!going_up);
            (extreme, first_at, last_at) = (v, i, i);
        }
    }
    if let Some(going_up) = up {
        moves.push((going_up, first_at.saturating_sub(left_at) as f64 / n as f64));
    }
    // A repeat's last move runs on into its first one when they go the same way.
    if repeats
        && moves.len() > 2
        && let (Some(first), Some(last)) = (moves.first().copied(), moves.last().copied())
        && first.0 == last.0
    {
        moves.pop();
        if let Some(m) = moves.first_mut() {
            m.1 += last.1;
        }
    }
    match moves.as_slice() {
        [(true, _)] => Shape::Rise,
        [(false, _)] => Shape::Fall,
        [a, b] => {
            let (rise, fall) = if a.0 { (a.1, b.1) } else { (b.1, a.1) };
            let both = (rise + fall).max(1e-9);
            // A saw: a jump back and a ramp all the rest of the way, with no hold: a rise or a fall.
            if fall / both < 0.25 && rise >= 0.9 && !smooth {
                Shape::Rise
            } else if fall / both < 0.25 {
                Shape::Pump
            } else if rise / both < 0.25 {
                Shape::Fall
            } else {
                Shape::UpDown
            }
        }
        _ => Shape::Other,
    }
}

/// Smooth when most of its sloped lines (by how far across they run) are bent.
fn feel(points: &[CurvePoint]) -> Feel {
    let (mut bent, mut all) = (0.0, 0.0);
    for w in points.windows(2) {
        let [a, b] = w else { continue };
        let width = b.x - a.x;
        if width <= 0.0 || (b.y - a.y).abs() < 0.02 {
            continue;
        }
        all += width;
        if a.bend.abs() >= 0.2 {
            bent += width;
        }
    }
    if all > 0.0 && bent / all >= 0.5 { Feel::Smooth } else { Feel::Sharp }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64, bend: f64) -> CurvePoint {
        CurvePoint { x, y, bend }
    }

    fn g(shape: Shape, rhythm: Rhythm, feel: Feel) -> Groups {
        Groups { shape, rhythm, rhythms: vec![rhythm], feel }
    }

    /// `times` copies of `one` (across 0 to 1) side by side.
    fn repeated(one: &[(f64, f64, f64)], times: usize) -> Vec<CurvePoint> {
        (0..times).flat_map(|k| one.iter().map(move |&(x, y, b)| p((k as f64 + x) / times as f64, y, b))).collect()
    }

    #[test]
    fn a_duck_four_times_a_bar_is_a_quarter_note_pump() {
        // Down at once, then back up, bent: a sidechain.
        let duck = repeated(&[(0.0, 1.0, 0.0), (0.0, 0.0, 0.6), (0.6, 1.0, 0.0), (1.0, 1.0, 0.0)], 4);
        assert_eq!(of(&duck, 4.0, false), g(Shape::Pump, Rhythm::Quarter, Feel::Smooth));
        // The same as a plugin's one-bar shape, once (it ends back at its start): a bar's pump. Without the drop back
        // it's a rise.
        let once = repeated(&[(0.0, 0.0, 0.6), (0.6, 1.0, 0.0), (1.0, 1.0, 0.0), (1.0, 0.0, 0.0)], 1);
        assert_eq!(of(&once[..3], 4.0, false).shape, Shape::Rise);
        assert_eq!(of(&once, 4.0, false).rhythm, Rhythm::Bar);
        assert_eq!(of(&once, 4.0, false).shape, Shape::Pump);
        // Sixteenths and eighth triplets.
        assert_eq!(
            of(&repeated(&[(0.0, 0.0, 0.6), (0.6, 1.0, 0.0), (1.0, 1.0, 0.0)], 16), 4.0, false).rhythm,
            Rhythm::Sixteenth
        );
        assert_eq!(
            of(&repeated(&[(0.0, 0.0, 0.6), (0.6, 1.0, 0.0), (1.0, 1.0, 0.0)], 12), 4.0, false).rhythm,
            Rhythm::EighthTriplet
        );
        assert_eq!(
            of(&repeated(&[(0.0, 0.0, 0.0), (0.6, 1.0, 0.0), (1.0, 1.0, 0.0)], 6), 4.0, false).rhythm,
            Rhythm::QuarterTriplet
        );
    }

    #[test]
    fn rises_falls_gates_steps_and_up_and_down() {
        let rise = [p(0.0, 0.1, 0.0), p(1.0, 0.9, 0.0)];
        assert_eq!(of(&rise, 64.0, false), g(Shape::Rise, Rhythm::Longer, Feel::Sharp));
        assert_eq!(of(&[p(0.0, 1.0, 0.5), p(1.0, 0.0, 0.0)], 2.0, false).shape, Shape::Fall);
        assert_eq!(of(&[p(0.0, 1.0, 0.5), p(1.0, 0.0, 0.0)], 2.0, false).rhythm, Rhythm::Half);
        // A pluck: up at once, then a slow drop.
        assert_eq!(of(&[p(0.0, 0.0, 0.0), p(0.05, 1.0, 0.0), p(1.0, 0.0, 0.0)], 3.0, false).shape, Shape::Fall);
        assert_eq!(of(&[p(0.0, 0.0, 0.0), p(0.05, 1.0, 0.0), p(1.0, 0.0, 0.0)], 3.0, false).rhythm, Rhythm::Free);
        let square = repeated(&[(0.0, 1.0, 0.0), (0.5, 1.0, 0.0), (0.5, 0.0, 0.0), (1.0, 0.0, 0.0)], 8);
        assert_eq!(of(&square, 4.0, false), g(Shape::Gate, Rhythm::Eighth, Feel::Sharp));
        let stairs = [
            p(0.0, 0.0, 0.0),
            p(0.25, 0.0, 0.0),
            p(0.25, 0.3, 0.0),
            p(0.5, 0.3, 0.0),
            p(0.5, 0.7, 0.0),
            p(0.75, 0.7, 0.0),
            p(0.75, 1.0, 0.0),
            p(1.0, 1.0, 0.0),
        ];
        assert_eq!(of(&stairs, 4.0, false).shape, Shape::Steps);
        let triangle = repeated(&[(0.0, 0.0, 0.0), (0.5, 1.0, 0.0), (1.0, 0.0, 0.0)], 2);
        assert_eq!(of(&triangle, 4.0, false), g(Shape::UpDown, Rhythm::Half, Feel::Sharp));
        // A sine-ish wobble: up and down, smooth.
        let wobble =
            repeated(&[(0.0, 0.0, 0.75), (0.25, 0.5, -0.75), (0.5, 1.0, 0.75), (0.75, 0.5, -0.75), (1.0, 0.0, 0.0)], 4);
        assert_eq!(of(&wobble, 4.0, false), g(Shape::UpDown, Rhythm::Quarter, Feel::Smooth));
        // A dotted eighth: three in 2.25 beats.
        let dotted = repeated(&[(0.0, 0.0, 0.0), (0.6, 1.0, 0.0), (1.0, 1.0, 0.0)], 3);
        assert_eq!(of(&dotted, 2.25, false).rhythm, Rhythm::DottedEighth);
        // Something busier is other.
        let busy = [
            p(0.0, 0.0, 0.0),
            p(0.1, 1.0, 0.0),
            p(0.2, 0.2, 0.0),
            p(0.45, 0.9, 0.0),
            p(0.5, 0.0, 0.0),
            p(0.9, 0.6, 0.0),
            p(1.0, 0.1, 0.0),
        ];
        assert_eq!(of(&busy, 4.0, false).shape, Shape::Other);
    }

    #[test]
    fn a_clip_with_straight_and_triplet_parts_is_mixed_and_found_under_each() {
        // A bar of 1/8 pumps (8), then a bar of 1/8 triplet pumps (12): two bars, 8 beats.
        let pump = [(0.0, 0.0, 0.6), (0.6, 1.0, 0.0), (1.0, 1.0, 0.0)];
        let mut points: Vec<CurvePoint> = repeated(&pump, 8).into_iter().map(|q| p(q.x / 2.0, q.y, q.bend)).collect();
        points.extend(repeated(&pump, 12).into_iter().map(|q| p(0.5 + q.x / 2.0, q.y, q.bend)));
        let groups = of(&points, 8.0, false);
        assert_eq!(groups.rhythm, Rhythm::Mixed);
        assert_eq!(groups.rhythms, [Rhythm::Eighth, Rhythm::EighthTriplet]);
        assert!(
            groups.goes_at(Rhythm::Eighth) && groups.goes_at(Rhythm::EighthTriplet) && groups.goes_at(Rhythm::Mixed)
        );
        assert!(!groups.goes_at(Rhythm::Quarter));
        assert_eq!(groups.shape, Shape::Pump);
        // A bar of 1/8 pumps then a flat bar: just 1/8.
        let mut half: Vec<CurvePoint> = repeated(&pump, 8).into_iter().map(|q| p(q.x / 2.0, q.y, q.bend)).collect();
        half.push(p(1.0, 1.0, 0.0));
        assert_eq!(of(&half, 8.0, false).rhythm, Rhythm::Eighth);
    }

    #[test]
    fn a_plugin_shape_loops_so_a_rise_that_stays_up_is_a_pump_and_a_sine_goes_up_and_down() {
        // Duck's default: up over half a bar, then held: as a loop it drops back at the start.
        let duck = [p(0.0, 0.0, 1.0), p(0.5, 1.0, 0.0), p(1.0, 1.0, 0.0)];
        assert_eq!(of(&duck, 4.0, true).shape, Shape::Pump);
        assert_eq!(of(&duck, 4.0, false).shape, Shape::Rise, "in a song it only rises");
        // A saw up stays a rise; a sine from the middle is up and down.
        assert_eq!(of(&[p(0.0, 0.0, 0.0), p(1.0, 1.0, 0.0)], 4.0, true).shape, Shape::Rise);
        let sine = [p(0.0, 0.5, -0.7), p(0.25, 1.0, 0.7), p(0.5, 0.5, -0.7), p(0.75, 0.0, 0.7), p(1.0, 0.5, 0.0)];
        assert_eq!(of(&sine, 4.0, true).shape, Shape::UpDown);
        // A duck with a short blip at its start isn't a fast rhythm.
        let blip = [p(0.0, 1.0, 0.0), p(0.01, 0.0, 0.0), p(0.02, 0.0, 0.5), p(0.5, 1.0, 0.0), p(1.0, 1.0, 0.0)];
        assert_eq!(of(&blip, 4.0, true).rhythm, Rhythm::Bar);
    }

    #[test]
    fn a_flat_or_empty_curve_is_other_and_free() {
        assert_eq!(of(&[], 4.0, false), Groups::default());
        assert_eq!(of(&[p(0.0, 0.5, 0.0), p(1.0, 0.5, 0.0)], 4.0, false).shape, Shape::Other);
        assert_eq!(of(&[p(0.0, 0.0, 0.0), p(1.0, 1.0, 0.0)], f64::NAN, false).rhythm, Rhythm::Free);
    }

    #[test]
    fn every_label_reads_back() {
        assert!(Shape::ALL.iter().all(|s| Shape::from_label(s.label()) == Some(*s)));
        assert!(Rhythm::ALL.iter().all(|r| Rhythm::from_label(r.label()) == Some(*r)));
        assert!(Feel::ALL.iter().all(|f| Feel::from_label(f.label()) == Some(*f)));
    }

    #[test]
    fn a_curve_is_drawn_with_its_bends() {
        let line = [p(0.0, 0.0, 1.0), p(1.0, 1.0, 0.0)];
        // The page's curve with its handle in the late corner: half way across it's (1 - 1/sqrt 2)^2 up.
        let late = (1.0 - 0.5f64.sqrt()).powi(2);
        assert!((value_at(&line, 0.5) - late).abs() < 1e-9, "a full late bend is low half way");
        let line = [p(0.0, 0.0, -1.0), p(1.0, 1.0, 0.0)];
        assert!((value_at(&line, 0.5) - (1.0 - late)).abs() < 1e-9);
        let line = [p(0.0, 0.0, 0.5), p(1.0, 1.0, 0.0)];
        assert!(value_at(&line, 0.5) < 0.5 && value_at(&line, 0.5) > late);
        // A broken bend is straight, as the page draws it.
        let line = [p(0.0, 0.0, f64::NAN), p(1.0, 1.0, 0.0)];
        assert!((value_at(&line, 0.3) - 0.3).abs() < 1e-9);
        // A jump: the second point wins; before the first and after the last, they hold.
        let jump = [p(0.2, 0.3, 0.0), p(0.5, 0.0, 0.0), p(0.5, 1.0, 0.0), p(0.8, 1.0, 0.0)];
        assert_eq!((value_at(&jump, 0.0), value_at(&jump, 0.5), value_at(&jump, 0.9)), (0.3, 1.0, 1.0));
    }
}
