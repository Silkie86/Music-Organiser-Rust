//! The curve editor's rules, with no screen: what each edit does to the curve, and undo.
//!
//! The curve being edited is one flat [`Shape`]: points in beats, each with the bend of the line to the next one,
//! over a length in beats. A **highlight** is a stretch of time ([`Span`]); **clips** are remembered highlights, each
//! knowing the curve it came from. Every edit is an [`Edit`] given to [`apply`], which gives a new shape (and where the
//! highlight is now) or `None` when the edit can't be done; nothing here changes a curve in place, so a drag is drawn
//! by applying its edit again and again to the shape as it was when the drag began ([`Editor::drag`]) and is one
//! step of undo however far it went.
//!
//! Lines bend as the page draws them ([`crate::curve_groups::value_at`]). A bent line cut in two can't always be
//! drawn exactly by two such lines, so each half is drawn again with the bend that puts its middle where it was, or
//! as a few lines where that isn't close enough ([`traced`]): the picture moves by less than a hundredth of its
//! height.
//!
//! A jump is two points at the same beat: the first is the value before it, the second the value from it on.

use mo_formats::bitwig::curve::CurvePoint;

use crate::curve_groups::bent;

/// Two beats this close are the same beat.
const EPS: f64 = 1e-9;
/// The longest curve the editor makes, in beats.
pub const MAX_BEATS: f64 = 256.0;
/// The shortest highlight, clip or curve, in beats (a 1/256 note).
pub const SHORTEST: f64 = 1.0 / 64.0;
/// How many steps back Undo goes.
pub const UNDO_STEPS: usize = 200;

/// One point of the curve being edited.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// When, in beats from the curve's start.
    pub beat: f64,
    /// The value, 0 to 1.
    pub value: f64,
    /// How the line to the next point bends, -1 to 1 (0 is straight).
    pub bend: f64,
}

impl Point {
    fn new(beat: f64, value: f64, bend: f64) -> Point {
        Point { beat, value, bend }
    }
}

/// A stretch of time, in beats: `start` is never after `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

impl Span {
    /// From `a` to `b`, whichever is first.
    pub fn new(a: f64, b: f64) -> Span {
        Span { start: a.min(b), end: a.max(b) }
    }

    pub fn len(self) -> f64 {
        self.end - self.start
    }

    /// Too short to hold anything.
    pub fn is_empty(self) -> bool {
        // A span with an edge that's not a number is empty too.
        let len = self.len();
        len.is_nan() || len < SHORTEST - EPS
    }

    fn same(self, other: Span) -> bool {
        same(self.start, other.start) && same(self.end, other.end)
    }

    fn overlaps(self, other: Span) -> bool {
        self.start < other.end - EPS && other.start < self.end - EPS
    }

    /// Kept inside 0 to `length`.
    fn within(self, length: f64) -> Span {
        Span { start: self.start.clamp(0.0, length), end: self.end.clamp(0.0, length) }
    }
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < EPS
}

/// A remembered highlight: a part of the curve with a name, and the curve from the list it came from when it did.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub span: Span,
    /// The id of the curve dropped here, if one was.
    pub source: Option<String>,
    pub name: String,
    /// Randomise leaves a pinned clip as it is.
    pub pinned: bool,
}

/// A curve from the list (or a made one), to drop into the curve being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    /// Its points, 0 to 1 across.
    pub points: Vec<CurvePoint>,
    pub length_beats: f64,
    pub source: Option<String>,
    pub name: String,
}

/// The curve being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    points: Vec<Point>,
    length: f64,
    clips: Vec<Clip>,
}

impl Shape {
    /// A curve of `points` over `length` beats, tidied: in order, inside the curve, values 0 to 1.
    pub fn new(points: Vec<Point>, length: f64) -> Shape {
        let length = if length.is_finite() { length.clamp(SHORTEST, MAX_BEATS) } else { 4.0 };
        Shape { points: tidy(points, length), length, clips: Vec::new() }
    }

    /// A curve from the list, to edit: its points (0 to 1 across) over its length.
    pub fn from_curve(points: &[CurvePoint], length_beats: f64) -> Shape {
        let length = if length_beats.is_finite() && length_beats > 0.0 { length_beats } else { 4.0 };
        let length = length.clamp(SHORTEST, MAX_BEATS);
        Shape::new(points.iter().map(|p| Point::new(p.x * length, p.y, p.bend)).collect(), length)
    }

    /// The curve as the list keeps one: its points 0 to 1 across.
    pub fn curve_points(&self) -> Vec<CurvePoint> {
        self.points.iter().map(|p| CurvePoint { x: p.beat / self.length, y: p.value, bend: p.bend }).collect()
    }

    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// The curve's length in beats.
    pub fn length(&self) -> f64 {
        self.length
    }

    /// The clips, in time order; none overlaps another.
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    /// The curve's points from its very start to its very end: where its first point comes later or its last one
    /// earlier, the value it holds there gets a point of its own.
    fn whole_points(&self) -> Vec<Point> {
        cut(&self.points, self.whole())
    }

    /// The whole curve as a stretch of time.
    pub fn whole(&self) -> Span {
        Span { start: 0.0, end: self.length }
    }

    /// The value at `beat` (at a jump, the value from it on).
    pub fn value_at(&self, beat: f64) -> f64 {
        value_at(&self.points, beat)
    }

    /// The value at `beat` on the line that starts at the point at `line` in [`Shape::points`] (its own value when
    /// it's the last point, or half of a jump).
    pub fn line_value(&self, line: usize, beat: f64) -> f64 {
        on_line(&self.points, line, beat)
    }

    /// The value just before `beat` (at a jump, the value before it).
    pub fn value_before(&self, beat: f64) -> f64 {
        value_before(&self.points, beat)
    }

    /// The lowest and highest value in `span`.
    pub fn bounds(&self, span: Span) -> (f64, f64) {
        let span = span.within(self.length);
        if span.is_empty() {
            let v = if span.start.is_finite() { self.value_at(span.start) } else { 0.0 };
            return (v, v);
        }
        bounds(&cut(&self.points, span))
    }
}

/// A grid line's spacing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
}

impl Step {
    pub const ALL: [Step; 4] = [Step::Quarter, Step::Eighth, Step::Sixteenth, Step::ThirtySecond];

    pub fn label(self) -> &'static str {
        match self {
            Step::Quarter => "1/4",
            Step::Eighth => "1/8",
            Step::Sixteenth => "1/16",
            Step::ThirtySecond => "1/32",
        }
    }

    fn beats(self) -> f64 {
        match self {
            Step::Quarter => 1.0,
            Step::Eighth => 0.5,
            Step::Sixteenth => 0.25,
            Step::ThirtySecond => 0.125,
        }
    }
}

/// The grid edits land on: a step (none is "Off"), straight, as triplets (two thirds of the step) or dotted (one and
/// a half of it; Solomon, 14 Nov 2026). Triplet wins if both are set: the screen sets one at most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub step: Option<Step>,
    pub triplet: bool,
    pub dotted: bool,
}

impl Default for Grid {
    fn default() -> Grid {
        Grid { step: Some(Step::Sixteenth), triplet: false, dotted: false }
    }
}

impl Grid {
    /// The beats between two grid lines; none when the grid is off.
    pub fn beats(self) -> Option<f64> {
        let step = self.step?.beats();
        Some(if self.triplet {
            step * 2.0 / 3.0
        } else if self.dotted {
            step * 1.5
        } else {
            step
        })
    }

    /// `beat` on its nearest grid line; as it is when `free` (Shift held) or the grid is off.
    pub fn snap(self, beat: f64, free: bool) -> f64 {
        match self.beats() {
            Some(step) if !free => (beat / step).round() * step,
            _ => beat,
        }
    }
}

/// One edit. Those of a highlight take the whole curve when nothing is highlighted, but for the ones that move time
/// (Move, Speed, Repeat, RemoveTime, Swap), which need one.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// A new point (double-click on empty space or a line).
    AddPoint {
        beat: f64,
        value: f64,
    },
    /// The point at this place in [`Shape::points`] goes (double-click on it).
    RemovePoint(usize),
    /// A point dragged: it stays between its neighbours.
    MovePoint {
        point: usize,
        beat: f64,
        value: f64,
    },
    /// The line from this point to the next bent (Alt-drag); 0 straightens it (Alt-double-click).
    Bend {
        line: usize,
        bend: f64,
    },
    /// The line from this point to the next moved up or down by `by`, as far as it can go.
    MoveLine {
        line: usize,
        by: f64,
    },

    /// The highlight dragged by its bar to start at `to`; `copy` (Ctrl) leaves what was there.
    Move {
        to: f64,
        copy: bool,
    },
    /// A side dragged: the highlight's part played over `start` to `end` instead, faster or slower; backwards when
    /// `start` is after `end` (a side dragged through the other). Time it no longer covers holds a value: after it,
    /// the one it ends on; before it, the one the curve reaches it with.
    Speed {
        start: f64,
        end: f64,
    },
    /// A side dragged with Ctrl. Outwards, the part repeats to fill `start` to `end`; inwards, what's left of it
    /// repeats to fill the highlight (a stutter).
    Repeat {
        start: f64,
        end: f64,
    },
    /// The top or bottom dragged: the part's lowest value goes to `low` and its highest to `high`; upside down
    /// when `low` is above `high`.
    Squash {
        low: f64,
        high: f64,
    },
    /// A corner dragged: the part fades, to `left` of its height at its start and `right` at its end (1 is all of
    /// it), measured from its lowest value, or from its highest when `from_top`.
    Tilt {
        left: f64,
        right: f64,
        from_top: bool,
    },
    /// The bar Alt-dragged: above 0 smoother (corners rounded, jumps made ramps), below 0 sharper (bends straighter,
    /// ramps shorter than `short` beats made jumps), to 1 or -1.
    Smooth {
        amount: f64,
        short: f64,
    },
    /// Upside down, within its own lowest and highest.
    Flip,
    /// Backwards.
    Reverse,
    /// Made into steps `step` beats long, each at the value in its middle.
    Steps {
        step: f64,
    },
    /// The part becomes a straight line from the value before it to the value after it.
    Flatten,
    /// The highlight's time is taken out: what follows moves up and the curve gets shorter.
    RemoveTime,

    /// A curve dropped between clips at `at`: what follows moves along and the curve gets longer.
    Insert {
        at: f64,
        piece: Piece,
    },
    /// A curve dropped on the highlight: it takes the part's place, fitted to its length, or at its own length
    /// (`keep_length`, Shift) with what follows moved.
    Swap {
        piece: Piece,
        keep_length: bool,
    },
    /// The highlight is remembered as a clip.
    Remember {
        name: String,
    },
    /// The clip at this place in [`Shape::clips`] is forgotten; the curve stays as it is.
    Forget(usize),
    Pin {
        clip: usize,
        pinned: bool,
    },
    /// The end mark dragged: the curve's length. Shorter cuts it; longer holds its last value.
    SetLength(f64),
}

/// `edit` done to `shape` with `highlight`: the new shape and its highlight, or None when it can't be done (no such
/// point, nothing highlighted for an edit that needs it, a part too short) or changes nothing.
pub fn apply(shape: &Shape, highlight: Option<Span>, edit: &Edit) -> Option<(Shape, Option<Span>)> {
    let highlight = highlight.map(|h| h.within(shape.length)).filter(|h| !h.is_empty());
    let (after, lit) = edited(shape, highlight, edit)?;
    let clips: Vec<Clip> = after
        .clips
        .iter()
        .map(|c| Clip { span: c.span.within(after.length), ..c.clone() })
        .filter(|c| !c.span.is_empty())
        .collect();
    let after = Shape { points: tidy(after.points, after.length), clips, ..after };
    let lit = lit.map(|h| h.within(after.length)).filter(|h| !h.is_empty());
    (after != *shape || lit != highlight).then_some((after, lit))
}

fn edited(shape: &Shape, highlight: Option<Span>, edit: &Edit) -> Option<(Shape, Option<Span>)> {
    let length = shape.length;
    let part = highlight.unwrap_or(shape.whole());
    let with = |points: Vec<Point>| Some((Shape { points, ..shape.clone() }, highlight));
    // The part's own points changed where they are.
    let reshaped = |f: &dyn Fn(Vec<Point>, f64) -> Option<Vec<Point>>| {
        let made = f(cut(&shape.points, part), part.len())?;
        with(put(&shape.points, part, &made))
    };
    let finite = |v: &[f64]| v.iter().all(|v| v.is_finite());
    match *edit {
        Edit::AddPoint { beat, value } => {
            if !finite(&[beat, value]) || !(0.0..=length).contains(&beat) {
                return None;
            }
            if shape.points.iter().any(|p| same(p.beat, beat)) {
                return None;
            }
            let mut points = shape.points.clone();
            split_at(&mut points, beat);
            points.iter_mut().find(|p| same(p.beat, beat))?.value = value.clamp(0.0, 1.0);
            with(points)
        }
        Edit::RemovePoint(i) => {
            if i >= shape.points.len() || shape.points.len() < 2 {
                return None;
            }
            let mut points = shape.points.clone();
            points.remove(i);
            with(points)
        }
        Edit::MovePoint { point, beat, value } => {
            if !finite(&[beat, value]) {
                return None;
            }
            let mut points = shape.points.clone();
            let low = point.checked_sub(1).and_then(|i| points.get(i)).map_or(0.0, |p| p.beat);
            let high = points.get(point + 1).map_or(length, |p| p.beat);
            let p = points.get_mut(point)?;
            p.beat = beat.clamp(low, high.max(low));
            p.value = value.clamp(0.0, 1.0);
            with(points)
        }
        Edit::Bend { line, bend } => {
            let mut points = shape.points.clone();
            points.get(line + 1)?;
            points.get_mut(line)?.bend = if bend.is_finite() { bend.clamp(-1.0, 1.0) } else { 0.0 };
            with(points)
        }
        Edit::MoveLine { line, by } => {
            let mut points = shape.points.clone();
            let (a, b) = (points.get(line)?.value, points.get(line + 1)?.value);
            if !by.is_finite() {
                return None;
            }
            let by = by.clamp(-a.min(b), 1.0 - a.max(b));
            for p in points.get_mut(line..=line + 1)? {
                p.value += by;
            }
            with(points)
        }

        Edit::Move { to, copy } => {
            let from = highlight?;
            if !to.is_finite() {
                return None;
            }
            let start = to.clamp(0.0, (length - from.len()).max(0.0));
            let there = Span { start, end: start + from.len() };
            // Let go where it was: nothing moved, nothing copied.
            if there.same(from) {
                return None;
            }
            let part = cut(&shape.points, from);
            let base = if copy { shape.points.clone() } else { held(&shape.points, from) };
            let points = put(&base, there, &part);
            let clips = clips_moved(&shape.clips, from, there, copy);
            Some((Shape { points, clips, length }, Some(there)))
        }
        Edit::Speed { start, end } => {
            let from = highlight?;
            if !finite(&[start, end]) {
                return None;
            }
            let there = Span::new(start, end).within(length);
            if there.is_empty() {
                return None;
            }
            if there.same(from) && start <= end {
                return None;
            }
            let mut part = cut(&shape.points, from);
            if start > end {
                part = reversed(&part, from.len());
            }
            let part = stretched(&part, there.len() / from.len());
            let mut points = put(&held(&shape.points, from), there, &part);
            // The time it no longer covers after it holds the value it ends on (before it, the value it's reached
            // with): nothing else moves.
            if let (true, Some(last)) = (there.end < from.end - EPS, part.last()) {
                let after = Span { start: there.end, end: from.end };
                let hold = [Point::new(0.0, last.value, 0.0), Point::new(after.len(), last.value, 0.0)];
                points = put(&points, after, &hold);
            }
            Some((Shape { points, clips: clips_moved(&shape.clips, from, there, false), length }, Some(there)))
        }
        Edit::Repeat { start, end } => {
            let from = highlight?;
            if !finite(&[start, end]) {
                return None;
            }
            let there = Span::new(start, end).within(length);
            if there.is_empty() {
                return None;
            }
            if there.same(from) {
                return None;
            }
            // The side that stayed is where the repeats are counted from.
            let from_end = same(there.end, from.end) && !same(there.start, from.start);
            let part = cut(&shape.points, from);
            if there.len() > from.len() + EPS {
                let points = put(&shape.points, there, &tiled(&part, from.len(), there.len(), from_end));
                let clips = clips_moved(&shape.clips, from, there, false);
                return Some((Shape { points, clips, length }, Some(there)));
            }
            let kept = if from_end {
                Span { start: from.len() - there.len(), end: from.len() }
            } else {
                Span { start: 0.0, end: there.len() }
            };
            let stutter = tiled(&cut(&part, kept), kept.len(), from.len(), from_end);
            with(put(&shape.points, from, &stutter))
        }
        Edit::Squash { low, high } => {
            if !finite(&[low, high]) {
                return None;
            }
            let (low, high) = (low.clamp(0.0, 1.0), high.clamp(0.0, 1.0));
            let (was_low, was_high) = bounds(&cut(&shape.points, part));
            if same(low, was_low) && same(high, was_high) {
                return None;
            }
            // Stretched taller, a bent line cut at the part's edge would show how roughly it was drawn again: it's
            // drawn that much closer first.
            let taller = ((high - low).abs() / (was_high - was_low).max(1e-9)).clamp(1.0, 64.0);
            let made = squashed(cut_close(&shape.points, part, CLOSE / taller), low, high);
            with(put(&shape.points, part, &made))
        }
        Edit::Flip => reshaped(&|part, _| {
            let (low, high) = bounds(&part);
            Some(squashed(part, high, low))
        }),
        Edit::Reverse => reshaped(&|part, len| Some(reversed(&part, len))),
        Edit::Tilt { left, right, from_top } => {
            if !finite(&[left, right]) {
                return None;
            }
            let (left, right) = (left.clamp(0.0, 1.0), right.clamp(0.0, 1.0));
            if left == 1.0 && right == 1.0 {
                return None;
            }
            reshaped(&|part, len| Some(tilted(&part, len, left, right, from_top)))
        }
        Edit::Smooth { amount, short } => {
            if !finite(&[amount, short]) || amount == 0.0 {
                return None;
            }
            let amount = amount.clamp(-1.0, 1.0);
            reshaped(&|part, _| {
                Some(if amount > 0.0 { smoothed(&part, amount) } else { sharpened(part, -amount, short) })
            })
        }
        Edit::Steps { step } => {
            if !step.is_finite() || step < SHORTEST {
                return None;
            }
            reshaped(&|part, len| Some(stepped(&part, len, step)))
        }
        Edit::Flatten => {
            let (before, after) = (shape.value_before(part.start), shape.value_at(part.end));
            with(put(&shape.points, part, &[Point::new(0.0, before, 0.0), Point::new(part.len(), after, 0.0)]))
        }
        Edit::RemoveTime => {
            let gone = highlight?;
            removed(shape, gone).map(|s| (s, None))
        }

        Edit::Insert { at, ref piece } => {
            if !at.is_finite() {
                return None;
            }
            inserted(shape, at.clamp(0.0, length), piece)
        }
        Edit::Swap { ref piece, keep_length } => {
            let over = highlight?;
            if keep_length {
                // Nothing left beside it (the whole curve, or all but a sliver): the new curve is all there is.
                return match removed(shape, over) {
                    Some(rest) => inserted(&rest, over.start, piece),
                    None => {
                        let v = shape.value_before(over.start);
                        let none = Shape { points: vec![Point::new(0.0, v, 0.0)], clips: Vec::new(), length: 0.0 };
                        inserted(&none, 0.0, piece)
                    }
                };
            }
            let fit = Shape::from_curve(&piece.points, over.len());
            let points = put(&shape.points, over, &fit.whole_points());
            let mut clips = room_made(&shape.clips, over);
            clips.push(clip_of(piece, over));
            Some((Shape { points, clips: in_order(clips), length }, Some(over)))
        }
        Edit::Remember { ref name } => {
            let span = highlight?;
            // Remembered already: only its name can change.
            let mut clips = shape.clips.clone();
            if let Some(c) = clips.iter_mut().find(|c| c.span.same(span)) {
                c.name = name.clone();
            } else {
                clips = room_made(&clips, span);
                clips.push(Clip { span, source: None, name: name.clone(), pinned: false });
            }
            Some((Shape { clips: in_order(clips), ..shape.clone() }, highlight))
        }
        Edit::Forget(i) => {
            let mut clips = shape.clips.clone();
            if i >= clips.len() {
                return None;
            }
            clips.remove(i);
            Some((Shape { clips, ..shape.clone() }, highlight))
        }
        Edit::Pin { clip, pinned } => {
            let mut clips = shape.clips.clone();
            clips.get_mut(clip)?.pinned = pinned;
            Some((Shape { clips, ..shape.clone() }, highlight))
        }
        Edit::SetLength(beats) => {
            if !beats.is_finite() {
                return None;
            }
            let new = beats.clamp(SHORTEST, MAX_BEATS);
            let points =
                if new < length { cut(&shape.points, Span { start: 0.0, end: new }) } else { shape.points.clone() };
            let clips = shape
                .clips
                .iter()
                .map(|c| Clip { span: c.span.within(new), ..c.clone() })
                .filter(|c| !c.span.is_empty())
                .collect();
            Some((Shape { points, clips, length: new }, highlight))
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Points
// ---------------------------------------------------------------------------------------------------------------

/// Points in order, inside the curve, with values 0 to 1 and whole bends. At one beat at most two stay (a jump: the
/// first and the last), one when they hold the same value (the later, whose bend is the next line's). A jump at the
/// curve's very start or end shows nothing, so its hidden side goes.
fn tidy(points: Vec<Point>, length: f64) -> Vec<Point> {
    let mut points: Vec<Point> = points
        .into_iter()
        .filter(|p| p.beat.is_finite() && p.value.is_finite())
        .map(|p| Point {
            beat: p.beat.clamp(0.0, length),
            value: p.value.clamp(0.0, 1.0),
            bend: if p.bend.is_finite() { p.bend.clamp(-1.0, 1.0) } else { 0.0 },
        })
        .collect();
    // Beats that are the same beat but for the last digits of the sum that made them become one beat, so the two
    // sides of a jump stay in the order they were given (a sort by the bare numbers could swap them).
    let mut beats: Vec<f64> = points.iter().map(|p| p.beat).collect();
    beats.sort_by(f64::total_cmp);
    let mut firsts: Vec<f64> = Vec::with_capacity(beats.len());
    for b in beats {
        if firsts.last().is_none_or(|f| b - f >= EPS) {
            firsts.push(b);
        }
    }
    for p in &mut points {
        let i = firsts.partition_point(|f| *f <= p.beat).saturating_sub(1);
        p.beat = firsts.get(i).copied().unwrap_or(p.beat);
    }
    points.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        let n = out.len();
        let at_same = |i: usize| out.get(i).is_some_and(|q| same(q.beat, p.beat));
        let third = n >= 2 && at_same(n - 1) && at_same(n - 2);
        let twin = n >= 1 && at_same(n - 1) && out.get(n - 1).is_some_and(|q| same(q.value, p.value));
        if third || twin {
            out.pop();
        }
        out.push(p);
    }
    // Only at the curve's own edges: a jump further in, with nothing before or after it, still says what value is
    // held up to it or from it on.
    if out.len() >= 2 && out[0].beat < EPS && same(out[0].beat, out[1].beat) {
        out.remove(0);
    }
    let n = out.len();
    if n >= 2 && out[n - 1].beat > length - EPS && same(out[n - 1].beat, out[n - 2].beat) {
        out.pop();
    }
    if let Some(last) = out.last_mut() {
        last.bend = 0.0;
    }
    if out.is_empty() {
        out.push(Point::new(0.0, 0.0, 0.0));
    }
    out
}

/// The value at `beat`: at a jump, the value from it on. Before the first point its value, after the last its value.
fn value_at(points: &[Point], beat: f64) -> f64 {
    let Some(i) = points.iter().rposition(|p| p.beat <= beat + EPS) else {
        return points.first().map_or(0.0, |p| p.value);
    };
    on_line(points, i, beat)
}

/// The value just before `beat`: at a jump, the value before it.
fn value_before(points: &[Point], beat: f64) -> f64 {
    match points.iter().position(|p| same(p.beat, beat)) {
        Some(i) => points[i].value,
        None => value_at(points, beat),
    }
}

/// The value at `beat` on the line that starts at point `i`.
fn on_line(points: &[Point], i: usize, beat: f64) -> f64 {
    let (Some(a), Some(b)) = (points.get(i), points.get(i + 1)) else {
        return points.get(i).map_or(0.0, |p| p.value);
    };
    let width = b.beat - a.beat;
    if width < EPS {
        return a.value;
    }
    a.value + (b.value - a.value) * bent(((beat - a.beat) / width).clamp(0.0, 1.0), a.bend)
}

/// The bend that puts a line's middle at `mid` of its rise (0 to 1): the other way round from [`bent`].
fn bend_for(mid: f64) -> f64 {
    // A line bent all the way reaches these at its middle.
    let (least, most) = (bent(0.5, 1.0), bent(0.5, -1.0));
    if !mid.is_finite() {
        return 0.0;
    }
    let mid = mid.clamp(least, most);
    // More bend, lower middle.
    let (mut low, mut high) = (-1.0f64, 1.0f64);
    for _ in 0..40 {
        let b = (low + high) / 2.0;
        if bent(0.5, b) > mid {
            low = b;
        } else {
            high = b;
        }
    }
    let b = (low + high) / 2.0;
    if b.abs() < 0.01 { 0.0 } else { b }
}

/// How far a line drawn in place of another may be from it, of the curve's height, before it's drawn as two.
const CLOSE: f64 = 0.01;

/// Draws what `f` gives (a value for a beat) from `a` to `b` (each a beat and its value) as bent lines, into `out`:
/// one line with the bend that puts its middle right, or, where that strays from `f` by more than `close`, two
/// lines that meet where it strayed most, each drawn the same way. The point at `b` isn't added.
fn traced(f: &dyn Fn(f64) -> f64, a: (f64, f64), b: (f64, f64), close: f64, depth: u32, out: &mut Vec<Point>) {
    let (width, rise) = (b.0 - a.0, b.1 - a.1);
    let bend = if rise.abs() < 1e-9 { 0.0 } else { bend_for((f((a.0 + b.0) / 2.0) - a.1) / rise) };
    // Where the one line is furthest from `f`, and by how much.
    let (worst, off) = (1..32)
        .map(|k| {
            let t = f64::from(k) / 32.0;
            (a.0 + t * width, (a.1 + rise * bent(t, bend) - f(a.0 + t * width)).abs())
        })
        .fold((a.0, 0.0), |far, at| if at.1 > far.1 { at } else { far });
    if off <= close || depth >= 7 || width < SHORTEST / 4.0 {
        out.push(Point::new(a.0, a.1, bend));
    } else {
        let m = (worst, f(worst).clamp(0.0, 1.0));
        traced(f, a, m, close, depth + 1, out);
        traced(f, m, b, close, depth + 1, out);
    }
}

/// Makes sure a point sits at `beat`, on the curve as it is. A straight line it's on is cut in two there; a bent
/// one is drawn again as two bent lines (or a few more, where it's bent hard), so the picture stays.
fn split_at(points: &mut Vec<Point>, beat: f64) {
    split_at_close(points, beat, CLOSE);
}

/// As [`split_at`], with how close a bent line drawn again must stay to itself.
fn split_at_close(points: &mut Vec<Point>, beat: f64, close: f64) {
    if points.iter().any(|p| same(p.beat, beat)) {
        return;
    }
    let i = points.partition_point(|p| p.beat < beat);
    let (before, after) = (i.checked_sub(1).and_then(|j| points.get(j)).copied(), points.get(i).copied());
    match (before, after) {
        (Some(a), Some(b)) => {
            let line = |x: f64| a.value + (b.value - a.value) * bent((x - a.beat) / (b.beat - a.beat), a.bend);
            let here = (beat, line(beat));
            let mut drawn = Vec::new();
            if a.bend.abs() < 0.01 {
                drawn.push(Point { bend: 0.0, ..a });
                drawn.push(Point::new(here.0, here.1, 0.0));
            } else {
                traced(&line, (a.beat, a.value), here, close, 0, &mut drawn);
                traced(&line, here, (b.beat, b.value), close, 0, &mut drawn);
            }
            points.splice(i - 1..i, drawn);
        }
        (None, Some(b)) => points.insert(0, Point::new(beat, b.value, 0.0)),
        (Some(a), None) => {
            points[i - 1].bend = 0.0;
            points.push(Point::new(beat, a.value, 0.0));
        }
        (None, None) => points.push(Point::new(beat, 0.0, 0.0)),
    }
}

/// The part of the curve in `span`, as points of its own counted from the span's start: from the value the span
/// starts with to the value it ends with (a jump on either edge stays outside).
fn cut(points: &[Point], span: Span) -> Vec<Point> {
    cut_close(points, span, CLOSE)
}

/// As [`cut`], with how close a bent line cut at an edge must stay to itself.
fn cut_close(points: &[Point], span: Span, close: f64) -> Vec<Point> {
    let mut all = points.to_vec();
    split_at_close(&mut all, span.start, close);
    split_at_close(&mut all, span.end, close);
    let first = all.iter().rposition(|p| same(p.beat, span.start)).unwrap_or(0);
    let last = all.iter().position(|p| same(p.beat, span.end)).unwrap_or(all.len().saturating_sub(1));
    let mut out: Vec<Point> = all
        .get(first..=last.max(first))
        .unwrap_or_default()
        .iter()
        .map(|p| Point { beat: (p.beat - span.start).clamp(0.0, span.len()), ..*p })
        .collect();
    if let Some(p) = out.last_mut() {
        p.bend = 0.0;
    }
    out
}

/// The curve with `part` (points counted from the span's start) in place of what was in `span`. Where its ends
/// don't meet the curve's values there's a jump.
fn put(points: &[Point], span: Span, part: &[Point]) -> Vec<Point> {
    let mut all = points.to_vec();
    split_at(&mut all, span.start);
    split_at(&mut all, span.end);
    let left = all.iter().position(|p| same(p.beat, span.start)).unwrap_or(0);
    let right = all.iter().rposition(|p| same(p.beat, span.end)).unwrap_or(all.len().saturating_sub(1));
    let mut out: Vec<Point> = all.get(..=left).unwrap_or_default().to_vec();
    // The point on the left edge only says what value the curve arrives with.
    if let Some(p) = out.last_mut() {
        p.bend = 0.0;
    }
    out.extend(part.iter().map(|p| Point { beat: span.start + p.beat.clamp(0.0, span.len()), ..*p }));
    if let Some(p) = out.last_mut() {
        p.bend = 0.0;
    }
    out.extend_from_slice(all.get(right..).unwrap_or_default());
    out
}

/// The curve with `span` held at the value it arrives with (what a part leaves behind when it moves away).
fn held(points: &[Point], span: Span) -> Vec<Point> {
    let v = value_before(points, span.start);
    put(points, span, &[Point::new(0.0, v, 0.0), Point::new(span.len(), v, 0.0)])
}

fn bounds(part: &[Point]) -> (f64, f64) {
    let (low, high) = part.iter().fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.value), h.max(p.value)));
    if low > high { (0.0, 0.0) } else { (low, high) }
}

// ---------------------------------------------------------------------------------------------------------------
// What a part can become
// ---------------------------------------------------------------------------------------------------------------

/// The part backwards. A line's bend leans the other way when it's walked from its other end.
fn reversed(part: &[Point], len: f64) -> Vec<Point> {
    let n = part.len();
    (0..n)
        .map(|i| {
            let p = part[n - 1 - i];
            let bend = if i + 1 < n { -part[n - 2 - i].bend } else { 0.0 };
            Point::new(len - p.beat, p.value, bend)
        })
        .collect()
}

/// The part `by` times as long.
fn stretched(part: &[Point], by: f64) -> Vec<Point> {
    part.iter().map(|p| Point { beat: p.beat * by, ..*p }).collect()
}

/// The part (`len` long) again and again over `total` beats: from the start, the last one cut short, or `from_end`,
/// the first one.
fn tiled(part: &[Point], len: f64, total: f64, from_end: bool) -> Vec<Point> {
    if len < SHORTEST - EPS {
        return part.to_vec();
    }
    let times = (total / len - 1e-9).ceil().max(1.0) as usize;
    let first = if from_end { total - times as f64 * len } else { 0.0 };
    let mut all = Vec::with_capacity(part.len() * times);
    for k in 0..times {
        all.extend(part.iter().map(|p| Point { beat: first + k as f64 * len + p.beat, ..*p }));
    }
    cut(&all, Span { start: 0.0, end: total })
}

/// The part with its lowest value at `low` and its highest at `high` (upside down when `low` is the higher). A flat
/// part goes to the middle of the two. Bends stay: a line's bend doesn't depend on how far it rises.
fn squashed(mut part: Vec<Point>, low: f64, high: f64) -> Vec<Point> {
    let (was_low, was_high) = bounds(&part);
    let span = was_high - was_low;
    for p in &mut part {
        p.value = if span < 1e-9 { (low + high) / 2.0 } else { low + (p.value - was_low) / span * (high - low) };
    }
    part
}

/// The part faded: its height over its lowest value (or under its highest, `from_top`) times a line from `left` at
/// its start to `right` at its end. Each line is drawn again as it lands (a straight line faded is a curve).
fn tilted(part: &[Point], len: f64, left: f64, right: f64, from_top: bool) -> Vec<Point> {
    let (low, high) = bounds(part);
    let floor = if from_top { high } else { low };
    let fade = |beat: f64, value: f64| {
        let t = if len > 0.0 { beat / len } else { 0.0 };
        floor + (value - floor) * (left + (right - left) * t)
    };
    let mut out = Vec::with_capacity(part.len());
    for (i, a) in part.iter().enumerate() {
        let start = (a.beat, fade(a.beat, a.value));
        match part.get(i + 1) {
            Some(b) if b.beat - a.beat > EPS => {
                let faded = |beat: f64| fade(beat, on_line(part, i, beat));
                traced(&faded, start, (b.beat, fade(b.beat, b.value)), CLOSE, 0, &mut out);
            }
            _ => out.push(Point::new(start.0, start.1, 0.0)),
        }
    }
    out
}

/// The bend of the line from `a` to `b` that takes it through `mid` at its middle. A level line can't bend.
fn fitted(a: Point, b: Point, mid: f64) -> f64 {
    let rise = b.value - a.value;
    if rise.abs() < 1e-9 { 0.0 } else { bend_for((mid - a.value) / rise) }
}

/// The line that starts at point `i`, from `from` to `to` beats along the part, into `out` (without its end): a
/// straight line as it is, a bent one drawn again.
fn line_part(points: &[Point], i: usize, from: f64, to: f64, out: &mut Vec<Point>) {
    let start = (from, on_line(points, i, from));
    if points.get(i).is_none_or(|p| p.bend.abs() < 0.01) {
        out.push(Point::new(start.0, start.1, 0.0));
    } else {
        traced(&|beat| on_line(points, i, beat), start, (to, on_line(points, i, to)), CLOSE, 0, out);
    }
}

/// The part smoother by `amount` (0 to 1): each jump becomes a ramp and each corner is rounded, over up to half of
/// the shorter line beside it. Away from them the curve is as it was. Each pass walks the points once.
fn smoothed(part: &[Point], amount: f64) -> Vec<Point> {
    let n = part.len();
    let wide = |points: &[Point], i: usize| match (points.get(i), points.get(i + 1)) {
        (Some(a), Some(b)) => b.beat - a.beat,
        _ => 0.0,
    };
    // Jumps first: a ramp from the curve a little before the jump to the curve a little after it. `ramp[i]` is how
    // far the jump whose first side is point `i` reaches each way.
    let mut ramp = vec![0.0; n];
    for i in 1..n.saturating_sub(2) {
        if same(part[i].beat, part[i + 1].beat) {
            let w = amount * 0.25 * wide(part, i - 1).min(wide(part, i + 1));
            if w > EPS {
                ramp[i] = w;
            }
        }
    }
    let mut ramps = Vec::with_capacity(n);
    for i in 0..n {
        let p = part[i];
        if ramp[i] > 0.0 {
            // The ramp's start, on the line before the jump; its end is where the line after the jump now starts.
            ramps.push(Point::new(p.beat - ramp[i], on_line(part, i - 1, p.beat - ramp[i]), 0.0));
            continue;
        }
        if wide(part, i) <= EPS {
            ramps.push(Point { bend: 0.0, ..p });
            continue;
        }
        let from = p.beat + if i >= 1 { ramp[i - 1] } else { 0.0 };
        let to = part[i + 1].beat - ramp.get(i + 1).copied().unwrap_or(0.0);
        line_part(part, i, from, to, &mut ramps);
    }
    // Corners: how far each point's rounding reaches along the lines beside it.
    let n = ramps.len();
    let reach: Vec<f64> = (0..n)
        .map(|i| {
            if i == 0 || i + 1 >= n {
                return 0.0;
            }
            let p = ramps[i];
            let r = amount * 0.5 * wide(&ramps, i - 1).min(wide(&ramps, i));
            if r <= EPS {
                return 0.0;
            }
            // Already smooth there: the corner is on the straight line between its two cut ends.
            let (v1, v2) = (on_line(&ramps, i - 1, p.beat - r), on_line(&ramps, i, p.beat + r));
            if (p.value - (v1 + v2) / 2.0).abs() < 1e-4 { 0.0 } else { r }
        })
        .collect();
    let mut out = Vec::with_capacity(n * 3);
    for i in 0..n {
        let (p, r) = (ramps[i], reach[i]);
        if r > 0.0 {
            let (v1, v2) = (on_line(&ramps, i - 1, p.beat - r), on_line(&ramps, i, p.beat + r));
            // A curve from the first cut end to the second, pulled toward the corner: its middle, and the middles of
            // its two halves.
            let top = (v1 + 2.0 * p.value + v2) / 4.0;
            let (first, last) = (Point::new(p.beat - r, v1, 0.0), Point::new(p.beat + r, v2, 0.0));
            let middle = Point::new(p.beat, top, 0.0);
            let in_mid = (v1 + (v1 + p.value) + top) / 4.0;
            let out_mid = (top + (p.value + v2) + v2) / 4.0;
            out.push(Point { bend: fitted(first, middle, in_mid), ..first });
            out.push(Point { bend: fitted(middle, last, out_mid), ..middle });
        }
        if wide(&ramps, i) <= EPS {
            out.push(Point { bend: 0.0, ..p });
            continue;
        }
        let to = ramps[i + 1].beat - reach.get(i + 1).copied().unwrap_or(0.0);
        line_part(&ramps, i, p.beat + r, to, &mut out);
    }
    out
}

/// The part sharper by `amount` (0 to 1): bends straighter, and a ramp shorter than `amount` of `short` beats
/// becomes a jump at its middle.
fn sharpened(mut part: Vec<Point>, amount: f64, short: f64) -> Vec<Point> {
    for p in &mut part {
        p.bend *= 1.0 - amount;
    }
    let (low, high) = bounds(&part);
    let n = part.len();
    for i in 0..n.saturating_sub(1) {
        let (a, b) = (part[i], part[i + 1]);
        let width = b.beat - a.beat;
        // Only a ramp that goes somewhere, and not one on the part's edge (its end must stay where it is).
        let inside = i > 0 && i + 2 < n;
        if inside && width > EPS && width < amount * short && (b.value - a.value).abs() > (high - low) * 0.1 {
            let mid = (a.beat + b.beat) / 2.0;
            part[i].beat = mid;
            part[i + 1].beat = mid;
            part[i].bend = 0.0;
        }
    }
    part
}

/// The part as steps `step` beats long, each at the value in its middle.
fn stepped(part: &[Point], len: f64, step: f64) -> Vec<Point> {
    let mut out = Vec::new();
    let mut start = 0.0;
    while start < len - EPS {
        let end = (start + step).min(len);
        let v = value_at(part, (start + end) / 2.0);
        out.push(Point::new(start, v, 0.0));
        out.push(Point::new(end, v, 0.0));
        start = end;
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Time put in and taken out, and the clips
// ---------------------------------------------------------------------------------------------------------------

/// `shape` with the time in `gone` taken out. None when nothing would be left.
fn removed(shape: &Shape, gone: Span) -> Option<Shape> {
    let length = shape.length - gone.len();
    if length < SHORTEST {
        return None;
    }
    let mut points = cut(&shape.points, Span { start: 0.0, end: gone.start });
    let rest = cut(&shape.points, Span { start: gone.end, end: shape.length });
    if gone.start < EPS {
        points.clear();
    }
    if shape.length - gone.end > EPS {
        points.extend(rest.iter().map(|p| Point { beat: gone.start + p.beat, ..*p }));
    }
    let clips = shape
        .clips
        .iter()
        .filter_map(|c| {
            // What's left of it on either side of the hole, joined up.
            let before = (c.span.start.min(gone.start), c.span.end.min(gone.start));
            let after = (c.span.start.max(gone.end), c.span.end.max(gone.end));
            let kept = (before.1 - before.0) + (after.1 - after.0);
            let start = if c.span.start <= gone.start { c.span.start } else { after.0 - gone.len() };
            let span = Span { start, end: start + kept };
            (!span.is_empty()).then(|| Clip { span, ..c.clone() })
        })
        .collect();
    Some(Shape { points, clips, length })
}

/// `piece` put in at `at`, at its own length: what follows moves along. A clip that `at` is inside is cut in two.
fn inserted(shape: &Shape, at: f64, piece: &Piece) -> Option<(Shape, Option<Span>)> {
    if !piece.length_beats.is_finite() || piece.length_beats < SHORTEST {
        return None;
    }
    let length = shape.length + piece.length_beats;
    if length > MAX_BEATS + EPS {
        return None;
    }
    let there = Span { start: at, end: at + piece.length_beats };
    let new = Shape::from_curve(&piece.points, piece.length_beats);
    let mut points = Vec::new();
    if at > EPS {
        points = cut(&shape.points, Span { start: 0.0, end: at });
    }
    points.extend(new.whole_points().iter().map(|p| Point { beat: at + p.beat, ..*p }));
    if shape.length - at > EPS {
        let rest = cut(&shape.points, Span { start: at, end: shape.length });
        points.extend(rest.iter().map(|p| Point { beat: there.end + p.beat, ..*p }));
    }
    let mut clips = Vec::new();
    for c in &shape.clips {
        let along = |b: f64| if b >= at - EPS { b + piece.length_beats } else { b };
        if c.span.start < at - EPS && c.span.end > at + EPS {
            clips.push(Clip { span: Span { start: c.span.start, end: at }, ..c.clone() });
            clips.push(Clip { span: Span { start: there.end, end: c.span.end + piece.length_beats }, ..c.clone() });
        } else if c.span.end <= at + EPS {
            clips.push(c.clone());
        } else {
            clips.push(Clip { span: Span { start: along(c.span.start), end: along(c.span.end) }, ..c.clone() });
        }
    }
    clips.retain(|c| !c.span.is_empty());
    clips.push(clip_of(piece, there));
    Some((Shape { points, clips: in_order(clips), length }, Some(there)))
}

fn clip_of(piece: &Piece, span: Span) -> Clip {
    Clip { span, source: piece.source.clone(), name: piece.name.clone(), pinned: false }
}

fn in_order(mut clips: Vec<Clip>) -> Vec<Clip> {
    clips.sort_by(|a, b| a.span.start.total_cmp(&b.span.start));
    clips
}

/// The clips with `span` free: one it covers goes, one it overlaps is cut back (one it sits inside keeps its
/// earlier side).
fn room_made(clips: &[Clip], span: Span) -> Vec<Clip> {
    clips
        .iter()
        .filter_map(|c| {
            if !c.span.overlaps(span) {
                return Some(c.clone());
            }
            let cut = if c.span.start < span.start - EPS {
                Span { start: c.span.start, end: span.start }
            } else {
                Span { start: span.end, end: c.span.end.max(span.end) }
            };
            (!cut.is_empty()).then(|| Clip { span: cut, ..c.clone() })
        })
        .collect()
}

/// The clips after the highlight went from `from` to `to`: a clip that was just the highlight goes with it (or
/// stays, with a twin there, for a copy), and the others make room for it.
fn clips_moved(clips: &[Clip], from: Span, to: Span, copy: bool) -> Vec<Clip> {
    // A highlight that's no clip leaves the clips as they are: they're only names for stretches of time.
    let Some(own) = clips.iter().position(|c| c.span.same(from)) else { return clips.to_vec() };
    let own = Some(own);
    let others: Vec<Clip> =
        clips.iter().enumerate().filter(|(i, _)| copy || Some(*i) != own).map(|(_, c)| c.clone()).collect();
    let mut out = room_made(&others, to);
    if let Some(c) = own.and_then(|i| clips.get(i)) {
        out.push(Clip { span: to, pinned: c.pinned && !copy, ..c.clone() });
    }
    in_order(out)
}

// ---------------------------------------------------------------------------------------------------------------
// The editor: the curve, its highlight, undo
// ---------------------------------------------------------------------------------------------------------------

type State = (Shape, Option<Span>);

/// A curve being edited, with its highlight and its steps of undo.
#[derive(Clone, Debug)]
pub struct Editor {
    now: State,
    pub grid: Grid,
    undo: Vec<State>,
    redo: Vec<State>,
    /// The curve as it was when the drag in hand began.
    drag_from: Option<State>,
    /// How many edits away from the curve as it was opened or last saved (None: it can't be got back to).
    from_saved: Option<i64>,
}

impl Editor {
    pub fn new(shape: Shape) -> Editor {
        Editor {
            now: (shape, None),
            grid: Grid::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            drag_from: None,
            from_saved: Some(0),
        }
    }

    pub fn shape(&self) -> &Shape {
        &self.now.0
    }

    pub fn highlight(&self) -> Option<Span> {
        self.now.1
    }

    /// Highlights a stretch of time (none, or one too short, takes the highlight off). Not a step of undo.
    pub fn set_highlight(&mut self, span: Option<Span>) {
        self.now.1 = span.map(|s| s.within(self.now.0.length)).filter(|s| !s.is_empty());
        // A drag in hand goes on from the curve as it began, with this highlight.
        if let Some(from) = &mut self.drag_from {
            from.1 = self.now.1;
        }
    }

    /// One edit, as one step of undo. False when it couldn't be done or changed nothing.
    pub fn edit(&mut self, edit: &Edit) -> bool {
        self.let_go();
        if self.drag(edit) {
            return self.let_go();
        }
        // Nothing is in hand after an edit that couldn't be done.
        self.drag_from = None;
        false
    }

    /// A drag moved: `edit` is done to the curve as it was when the drag began (the first call begins it), so the
    /// picture follows the mouse. False when the edit can't be done there: the curve shows as the drag began.
    pub fn drag(&mut self, edit: &Edit) -> bool {
        let from = self.drag_from.get_or_insert_with(|| self.now.clone());
        match apply(&from.0, from.1, edit) {
            Some(after) => {
                self.now = after;
                true
            }
            None => {
                self.now = from.clone();
                false
            }
        }
    }

    /// The drag ended: what it did is one step of undo. False when it changed nothing.
    pub fn let_go(&mut self) -> bool {
        let Some(from) = self.drag_from.take() else { return false };
        if from.0 == self.now.0 {
            return false;
        }
        self.undo.push(from);
        if self.undo.len() > UNDO_STEPS {
            self.undo.remove(0);
            // The saved curve may have been the one that fell off the end.
            if self.from_saved.is_some_and(|n| n >= UNDO_STEPS as i64) {
                self.from_saved = None;
            }
        }
        // A new edit after undoing: the undone ones (and a saved curve among them) can't be got back to.
        if self.from_saved.is_some_and(|n| n < 0) {
            self.from_saved = None;
        }
        self.redo.clear();
        self.from_saved = self.from_saved.map(|n| n + 1);
        true
    }

    /// The drag given up (Esc): the curve is as it was when it began.
    pub fn give_up(&mut self) -> bool {
        match self.drag_from.take() {
            Some(from) => {
                self.now = from;
                true
            }
            None => false,
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag_from.is_some()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        self.give_up();
        let Some(before) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(&mut self.now, before));
        self.from_saved = self.from_saved.map(|n| n - 1);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.give_up();
        let Some(after) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(&mut self.now, after));
        self.from_saved = self.from_saved.map(|n| n + 1);
        true
    }

    /// Whether the curve differs from the one opened or last saved.
    pub fn changed(&self) -> bool {
        self.from_saved != Some(0)
    }

    /// The curve was saved as it is now.
    pub fn saved(&mut self) {
        self.let_go();
        self.from_saved = Some(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(beat: f64, value: f64) -> Point {
        Point::new(beat, value, 0.0)
    }

    /// A triangle over 4 beats: up to 1 at beat 2, back down.
    fn triangle() -> Shape {
        Shape::new(vec![pt(0.0, 0.0), pt(2.0, 1.0), pt(4.0, 0.0)], 4.0)
    }

    /// A ramp over 4 beats.
    fn ramp() -> Shape {
        Shape::new(vec![pt(0.0, 0.0), pt(4.0, 1.0)], 4.0)
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// The two shapes draw the same picture, to within `by` of the height.
    fn same_picture(a: &Shape, b: &Shape, by: f64) {
        assert!(near(a.length(), b.length()), "lengths {} and {}", a.length(), b.length());
        for i in 0..=400 {
            // Between the samples of a jump, so a jump's two sides aren't compared with each other.
            let beat = (i as f64 + 0.37) / 401.0 * a.length();
            let (x, y) = (a.value_at(beat), b.value_at(beat));
            assert!((x - y).abs() <= by, "at beat {beat}: {x} and {y}");
        }
    }

    fn done(shape: &Shape, highlight: Option<Span>, edit: Edit) -> (Shape, Option<Span>) {
        apply(shape, highlight, &edit).expect("the edit can be done")
    }

    #[test]
    fn a_shape_is_tidied() {
        let s = Shape::new(
            vec![
                pt(3.0, 2.0),
                pt(1.0, 0.5),
                pt(f64::NAN, 0.5),
                pt(1.0, 0.5),
                pt(9.0, 0.25),
                Point::new(0.0, -1.0, 7.0),
            ],
            4.0,
        );
        let beats: Vec<f64> = s.points().iter().map(|p| p.beat).collect();
        assert_eq!(beats, [0.0, 1.0, 3.0, 4.0]);
        assert_eq!(s.points()[0].value, 0.0);
        assert_eq!(s.points()[0].bend, 1.0);
        assert_eq!(s.points()[2].value, 1.0);
        // Three at one beat: the first and the last stay, as a jump.
        let s = Shape::new(vec![pt(0.0, 0.0), pt(1.0, 0.1), pt(1.0, 0.2), pt(1.0, 0.3), pt(2.0, 1.0)], 2.0);
        let values: Vec<f64> = s.points().iter().map(|p| p.value).collect();
        assert_eq!(values, [0.0, 0.1, 0.3, 1.0]);
        assert_eq!(Shape::new(Vec::new(), 4.0).points().len(), 1);
    }

    #[test]
    fn a_curve_from_the_list_comes_back_the_same() {
        let points = vec![
            CurvePoint { x: 0.0, y: 0.0, bend: 0.5 },
            CurvePoint { x: 0.5, y: 1.0, bend: -0.3 },
            CurvePoint { x: 1.0, y: 0.25, bend: 0.0 },
        ];
        let back = Shape::from_curve(&points, 8.0).curve_points();
        assert_eq!(back, points);
    }

    #[test]
    fn a_jump_has_a_value_before_and_from_it() {
        let s = Shape::new(vec![pt(0.0, 0.2), pt(1.0, 0.2), pt(1.0, 0.9), pt(2.0, 0.9)], 2.0);
        assert!(near(s.value_before(1.0), 0.2));
        assert!(near(s.value_at(1.0), 0.9));
        assert!(near(s.value_at(0.999), 0.2));
    }

    #[test]
    fn the_bend_found_for_a_middle_gives_that_middle() {
        for bend in [-1.0, -0.6, -0.2, 0.3, 0.75, 1.0] {
            assert!((bend_for(bent(0.5, bend)) - bend).abs() < 1e-6, "bend {bend}");
        }
        assert_eq!(bend_for(0.5), 0.0);
    }

    #[test]
    fn a_bent_line_cut_in_two_keeps_its_picture() {
        for bend in [-1.0, -0.9, -0.4, 0.5, 0.8, 1.0] {
            let whole = Shape::new(vec![Point::new(0.0, 0.1, bend), pt(4.0, 0.9)], 4.0);
            for at in [0.5, 1.0, 2.0, 3.3] {
                let (cut, _) = done(&whole, None, Edit::AddPoint { beat: at, value: whole.value_at(at) });
                // A gentle bend needs only the new point; a hard one a few more to stay the same picture.
                let most = if bend.abs() <= 0.5 { 3 } else { 9 };
                assert!((3..=most).contains(&cut.points().len()), "bend {bend}: {} points", cut.points().len());
                same_picture(&whole, &cut, 0.012);
            }
        }
    }

    #[test]
    fn points_are_added_moved_and_removed() {
        let (s, _) = done(&ramp(), None, Edit::AddPoint { beat: 1.0, value: 0.9 });
        assert_eq!(s.points().len(), 3);
        assert!(near(s.value_at(1.0), 0.9));
        // No second point at the same beat by a double-click.
        assert!(apply(&s, None, &Edit::AddPoint { beat: 1.0, value: 0.1 }).is_none());
        assert!(apply(&s, None, &Edit::AddPoint { beat: 5.0, value: 0.1 }).is_none());
        // A point stays between its neighbours and inside 0 to 1.
        let (m, _) = done(&s, None, Edit::MovePoint { point: 1, beat: 9.0, value: 2.0 });
        assert!(near(m.points()[1].beat, 4.0));
        assert!(near(m.points()[1].value, 1.0));
        let (m, _) = done(&s, None, Edit::MovePoint { point: 1, beat: -3.0, value: 0.5 });
        // Dragged onto the curve's start, it is the start: a jump there would show nothing.
        assert_eq!(m.points().len(), 2);
        assert!(near(m.points()[0].beat, 0.0) && near(m.points()[0].value, 0.5));
        let (r, _) = done(&s, None, Edit::RemovePoint(1));
        assert_eq!(r, ramp());
        assert!(apply(&s, None, &Edit::RemovePoint(7)).is_none());
        assert!(apply(&s, None, &Edit::MovePoint { point: 7, beat: 1.0, value: 0.5 }).is_none());
    }

    #[test]
    fn a_line_bends_and_moves_as_far_as_it_can() {
        let (b, _) = done(&ramp(), None, Edit::Bend { line: 0, bend: 3.0 });
        assert_eq!(b.points()[0].bend, 1.0);
        assert!(b.value_at(2.0) < 0.2);
        assert!(apply(&ramp(), None, &Edit::Bend { line: 1, bend: 0.5 }).is_none(), "the last point starts no line");
        let s = Shape::new(vec![pt(0.0, 0.2), pt(2.0, 0.6), pt(4.0, 0.0)], 4.0);
        let (m, _) = done(&s, None, Edit::MoveLine { line: 0, by: 0.9 });
        assert!(near(m.points()[0].value, 0.6));
        assert!(near(m.points()[1].value, 1.0));
        let (m, _) = done(&s, None, Edit::MoveLine { line: 0, by: -0.9 });
        assert!(near(m.points()[0].value, 0.0));
        assert!(near(m.points()[1].value, 0.4));
    }

    #[test]
    fn reverse_twice_and_flip_twice_give_the_curve_back() {
        let s = Shape::new(
            vec![Point::new(0.0, 0.1, 0.6), pt(1.0, 0.8), pt(1.0, 0.3), Point::new(2.5, 0.5, -0.4), pt(4.0, 1.0)],
            4.0,
        );
        for edit in [Edit::Reverse, Edit::Flip] {
            for lit in [None, Some(Span::new(0.5, 3.0))] {
                let (once, lit_after) = done(&s, lit, edit.clone());
                assert_eq!(lit_after, lit);
                let (twice, _) = done(&once, lit, edit.clone());
                same_picture(&s, &twice, 0.02);
            }
        }
    }

    #[test]
    fn reverse_plays_the_part_backwards_and_leaves_the_rest() {
        let s = ramp();
        let lit = Span::new(1.0, 3.0);
        let (r, _) = done(&s, Some(lit), Edit::Reverse);
        assert!(near(r.value_at(0.5), 0.125));
        assert!(near(r.value_at(1.5), 0.625), "1.5 plays what was at 2.5");
        assert!(near(r.value_at(2.5), 0.375));
        assert!(near(r.value_at(3.5), 0.875));
        // The joins are jumps.
        assert!(near(r.value_before(1.0), 0.25));
        assert!(near(r.value_at(1.0), 0.75));
    }

    #[test]
    fn a_reversed_bend_keeps_its_picture() {
        let s = Shape::new(vec![Point::new(0.0, 0.0, 0.8), pt(4.0, 1.0)], 4.0);
        let (r, _) = done(&s, None, Edit::Reverse);
        for beat in [0.5, 1.0, 2.0, 3.0, 3.5] {
            assert!(near(r.value_at(beat), s.value_at(4.0 - beat)), "at {beat}");
        }
    }

    #[test]
    fn flip_turns_the_part_over_within_its_own_height() {
        let s = Shape::new(vec![pt(0.0, 0.2), pt(2.0, 0.6), pt(4.0, 0.2)], 4.0);
        let (f, _) = done(&s, None, Edit::Flip);
        assert!(near(f.value_at(0.0), 0.6));
        assert!(near(f.value_at(2.0), 0.2));
    }

    #[test]
    fn squash_sets_the_parts_lowest_and_highest() {
        let (s, _) = done(&triangle(), None, Edit::Squash { low: 0.25, high: 0.75 });
        assert!(near(s.value_at(0.0), 0.25));
        assert!(near(s.value_at(2.0), 0.75));
        assert!(near(s.value_at(1.0), 0.5));
        // Dragged through: upside down.
        let (s, _) = done(&triangle(), None, Edit::Squash { low: 1.0, high: 0.5 });
        assert!(near(s.value_at(0.0), 1.0));
        assert!(near(s.value_at(2.0), 0.5));
        // Only the highlight's part, measured on its own height.
        let (s, _) = done(&triangle(), Some(Span::new(0.0, 1.0)), Edit::Squash { low: 0.0, high: 1.0 });
        assert!(near(s.value_before(1.0), 1.0));
        assert!(near(s.value_at(1.0), 0.5));
    }

    #[test]
    fn move_carries_the_part_and_leaves_a_hold_behind() {
        let s = triangle();
        let (m, lit) = done(&s, Some(Span::new(0.0, 1.0)), Edit::Move { to: 3.0, copy: false });
        assert_eq!(lit, Some(Span::new(3.0, 4.0)));
        assert!(near(m.value_at(0.5), 0.0), "what it left holds the value before it");
        assert!(near(m.value_at(3.5), 0.25), "the part plays there");
        assert!(near(m.value_at(2.0), 1.0), "the rest is as it was");
        // A copy leaves the part where it was too.
        let (c, _) = done(&s, Some(Span::new(0.0, 1.0)), Edit::Move { to: 3.0, copy: true });
        assert!(near(c.value_at(0.5), 0.25));
        assert!(near(c.value_at(3.5), 0.25));
        // It can't leave the curve.
        let (m, lit) = done(&s, Some(Span::new(0.0, 1.0)), Edit::Move { to: 99.0, copy: false });
        assert_eq!(lit, Some(Span::new(3.0, 4.0)));
        assert!(near(m.length(), 4.0));
        assert!(apply(&s, None, &Edit::Move { to: 1.0, copy: false }).is_none());
    }

    #[test]
    fn speed_plays_the_part_over_another_stretch() {
        let s = triangle();
        // Its first half at half speed: over the whole curve.
        let (slow, lit) = done(&s, Some(Span::new(0.0, 2.0)), Edit::Speed { start: 0.0, end: 4.0 });
        assert_eq!(lit, Some(Span::new(0.0, 4.0)));
        assert!(near(slow.value_at(2.0), 0.5));
        assert!(near(slow.value_before(4.0), 1.0));
        // At double speed.
        let (fast, lit) = done(&s, Some(Span::new(0.0, 2.0)), Edit::Speed { start: 0.0, end: 1.0 });
        assert_eq!(lit, Some(Span::new(0.0, 1.0)));
        assert!(near(fast.value_at(0.5), 0.5));
        assert!(near(fast.value_at(1.5), 1.0), "the time it left holds the value it ends on");
        assert!(near(fast.value_at(3.0), 0.5));
        // A side dragged through the other: backwards.
        let (back, lit) = done(&s, Some(Span::new(0.0, 2.0)), Edit::Speed { start: 2.0, end: 0.0 });
        assert_eq!(lit, Some(Span::new(0.0, 2.0)));
        assert!(near(back.value_at(0.5), 0.75));
        assert!(apply(&s, Some(Span::new(0.0, 2.0)), &Edit::Speed { start: 1.0, end: 1.0 }).is_none());
    }

    #[test]
    fn repeat_fills_outwards_and_stutters_inwards() {
        let s = Shape::new(vec![pt(0.0, 0.0), pt(1.0, 1.0), pt(1.0, 0.5), pt(8.0, 0.5)], 8.0);
        let lit = Span::new(0.0, 1.0);
        // Outwards: the ramp three and a half times.
        let (r, after) = done(&s, Some(lit), Edit::Repeat { start: 0.0, end: 3.5 });
        assert_eq!(after, Some(Span::new(0.0, 3.5)));
        for beat in [0.25, 1.25, 2.25, 3.25] {
            assert!(near(r.value_at(beat), 0.25), "at {beat}: {}", r.value_at(beat));
        }
        assert!(near(r.value_at(3.75), 0.5), "past the repeats the curve is as it was");
        // Inwards: its first quarter four times, in the same highlight.
        let (st, after) = done(&s, Some(lit), Edit::Repeat { start: 0.0, end: 0.25 });
        assert_eq!(after, Some(lit));
        for beat in [0.125, 0.375, 0.625, 0.875] {
            assert!(near(st.value_at(beat), 0.125), "at {beat}: {}", st.value_at(beat));
        }
        assert!(near(st.value_at(2.0), 0.5));
        // From the other side: its last quarter, counted from the end.
        let (st, _) = done(&s, Some(lit), Edit::Repeat { start: 0.75, end: 1.0 });
        for beat in [0.125, 0.375, 0.625, 0.875] {
            assert!(near(st.value_at(beat), 0.875), "at {beat}: {}", st.value_at(beat));
        }
    }

    #[test]
    fn tilt_fades_the_part() {
        let flat = Shape::new(vec![pt(0.0, 0.0), pt(0.0, 1.0), pt(4.0, 1.0), pt(4.0, 0.0)], 4.0);
        let s =
            Shape::new(vec![pt(0.0, 0.0), pt(1.0, 0.0), pt(1.0, 1.0), pt(3.0, 1.0), pt(3.0, 0.0), pt(4.0, 0.0)], 4.0);
        // A fade out: all of it at the start, none at the end.
        let (t, _) = done(&s, None, Edit::Tilt { left: 1.0, right: 0.0, from_top: false });
        assert!(near(t.value_at(1.0), 0.75));
        assert!(near(t.value_at(2.0), 0.5));
        assert!(near(t.value_before(3.0), 0.25));
        assert!(near(t.value_at(3.5), 0.0));
        // From the top: the low parts rise to it.
        let (t, _) = done(&s, None, Edit::Tilt { left: 0.0, right: 1.0, from_top: true });
        assert!(near(t.value_at(0.0), 1.0));
        assert!(near(t.value_at(3.5), 0.125));
        assert_eq!(flat.points().len(), 2, "a jump at the very start or end shows nothing");
    }

    #[test]
    fn steps_take_the_value_in_each_steps_middle() {
        let (s, _) = done(&ramp(), None, Edit::Steps { step: 1.0 });
        for (beat, v) in [(0.5, 0.125), (1.1, 0.375), (2.9, 0.625), (3.5, 0.875)] {
            assert!(near(s.value_at(beat), v), "at {beat}: {}", s.value_at(beat));
        }
        // Only the highlight; a last step cut short takes its own middle.
        let (s, _) = done(&ramp(), Some(Span::new(0.0, 1.5)), Edit::Steps { step: 1.0 });
        assert!(near(s.value_at(1.25), 0.3125));
        assert!(near(s.value_at(2.0), 0.5));
        assert!(apply(&ramp(), None, &Edit::Steps { step: 0.0 }).is_none());
    }

    #[test]
    fn smooth_rounds_corners_and_ramps_jumps_without_leaving_the_curves_range() {
        let (s, _) = done(&triangle(), None, Edit::Smooth { amount: 1.0, short: 0.25 });
        assert!(s.value_at(2.0) < 0.9 && s.value_at(2.0) > 0.4, "the peak is rounded: {}", s.value_at(2.0));
        assert!(near(s.value_at(0.0), 0.0) && near(s.value_at(4.0), 0.0), "its ends stay");
        assert!(near(s.value_at(0.5), 0.25), "away from the corner the line is as it was");
        // Half as much rounds less.
        let (half, _) = done(&triangle(), None, Edit::Smooth { amount: 0.5, short: 0.25 });
        assert!(half.value_at(2.0) > s.value_at(2.0));
        // A gate's jumps become ramps.
        let gate = Shape::new(vec![pt(0.0, 0.0), pt(2.0, 0.0), pt(2.0, 1.0), pt(4.0, 1.0)], 4.0);
        let (g, _) = done(&gate, None, Edit::Smooth { amount: 1.0, short: 0.25 });
        assert!(near(g.value_at(2.0), 0.5), "half way up at the jump: {}", g.value_at(2.0));
        assert!(near(g.value_at(1.0), 0.0) && near(g.value_at(3.0), 1.0));
        for i in 0..=80 {
            let (a, b) = (g.value_at(i as f64 / 20.0), g.value_at((i + 1) as f64 / 20.0));
            assert!(b >= a - 1e-9, "it only rises");
        }
        assert!(apply(&gate, None, &Edit::Smooth { amount: 0.0, short: 0.25 }).is_none());
    }

    #[test]
    fn sharpen_straightens_bends_and_turns_short_ramps_into_jumps() {
        let s = Shape::new(vec![Point::new(0.0, 0.0, 0.8), pt(2.0, 0.5), pt(2.1, 1.0), pt(4.0, 1.0)], 4.0);
        let (h, _) = done(&s, None, Edit::Smooth { amount: -0.5, short: 0.25 });
        assert!(near(h.points()[0].bend, 0.4));
        assert!(near(h.points()[1].beat, 2.05) && near(h.points()[2].beat, 2.05), "the short ramp is a jump");
        let (h, _) = done(&s, None, Edit::Smooth { amount: -1.0, short: 0.05 });
        assert_eq!(h.points()[0].bend, 0.0);
        assert!(near(h.points()[2].beat, 2.1), "a ramp longer than short stays");
    }

    #[test]
    fn flatten_draws_a_line_across_the_part() {
        let (f, _) = done(&triangle(), Some(Span::new(1.0, 3.0)), Edit::Flatten);
        assert!(near(f.value_at(2.0), 0.5));
        assert!(near(f.value_at(0.5), 0.25));
    }

    #[test]
    fn remove_time_closes_the_gap() {
        let mut s = triangle();
        s.clips = vec![
            Clip { span: Span::new(0.0, 1.0), source: None, name: "a".into(), pinned: false },
            Clip { span: Span::new(1.5, 2.5), source: None, name: "b".into(), pinned: false },
            Clip { span: Span::new(3.0, 4.0), source: None, name: "c".into(), pinned: true },
        ];
        let (r, lit) = done(&s, Some(Span::new(1.0, 3.0)), Edit::RemoveTime);
        assert_eq!(lit, None);
        assert!(near(r.length(), 2.0));
        assert!(near(r.value_at(0.5), 0.25));
        assert!(near(r.value_at(1.5), 0.25));
        let spans: Vec<Span> = r.clips().iter().map(|c| c.span).collect();
        assert_eq!(spans, [Span::new(0.0, 1.0), Span::new(1.0, 2.0)]);
        assert!(r.clips()[1].pinned);
        assert!(apply(&s, Some(s.whole()), &Edit::RemoveTime).is_none(), "something must be left");
        assert!(apply(&s, None, &Edit::RemoveTime).is_none());
    }

    fn saw() -> Piece {
        Piece {
            points: vec![CurvePoint { x: 0.0, y: 1.0, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.0, bend: 0.0 }],
            length_beats: 2.0,
            source: Some("saw-id".into()),
            name: "Saw".into(),
        }
    }

    #[test]
    fn a_dropped_curve_goes_in_between_and_moves_the_rest_along() {
        let mut s = triangle();
        s.clips = vec![Clip { span: Span::new(1.0, 3.0), source: None, name: "peak".into(), pinned: false }];
        let (i, lit) = done(&s, None, Edit::Insert { at: 2.0, piece: saw() });
        assert_eq!(lit, Some(Span::new(2.0, 4.0)));
        assert!(near(i.length(), 6.0));
        assert!(near(i.value_at(1.0), 0.5));
        assert!(near(i.value_at(3.0), 0.5), "the saw, half way down");
        assert!(near(i.value_at(5.0), 0.5), "what followed, two beats later");
        // The clip it landed inside is cut in two around it.
        let spans: Vec<Span> = i.clips().iter().map(|c| c.span).collect();
        assert_eq!(spans, [Span::new(1.0, 2.0), Span::new(2.0, 4.0), Span::new(4.0, 5.0)]);
        assert_eq!(i.clips()[1].source.as_deref(), Some("saw-id"));
        // At the very start and the very end.
        let (first, _) = done(&triangle(), None, Edit::Insert { at: 0.0, piece: saw() });
        assert!(near(first.value_at(0.0), 1.0) && near(first.value_at(4.0), 1.0));
        let (last, _) = done(&triangle(), None, Edit::Insert { at: 4.0, piece: saw() });
        assert!(near(last.value_at(4.0), 1.0) && near(last.value_at(5.0), 0.5));
        // Never past the longest curve.
        let long = Piece { length_beats: MAX_BEATS, ..saw() };
        assert!(apply(&triangle(), None, &Edit::Insert { at: 0.0, piece: long }).is_none());
    }

    #[test]
    fn a_curve_dropped_on_the_highlight_takes_its_place() {
        let s = triangle();
        let lit = Span::new(1.0, 2.0);
        let (fit, after) = done(&s, Some(lit), Edit::Swap { piece: saw(), keep_length: false });
        assert_eq!(after, Some(lit));
        assert!(near(fit.length(), 4.0));
        assert!(near(fit.value_at(1.5), 0.5), "the saw fitted into one beat");
        assert!(near(fit.value_at(3.0), 0.5));
        assert_eq!(fit.clips().len(), 1);
        assert_eq!(fit.clips()[0].span, lit);
        // At its own length: the rest moves.
        let (own, after) = done(&s, Some(lit), Edit::Swap { piece: saw(), keep_length: true });
        assert_eq!(after, Some(Span::new(1.0, 3.0)));
        assert!(near(own.length(), 5.0));
        assert!(near(own.value_at(2.0), 0.5));
        assert!(near(own.value_at(4.0), 0.5), "what was at beat 3");
        // Over the whole curve.
        let (all, _) = done(&s, Some(s.whole()), Edit::Swap { piece: saw(), keep_length: true });
        assert!(near(all.length(), 2.0));
        assert!(near(all.value_at(1.0), 0.5));
        assert!(apply(&s, None, &Edit::Swap { piece: saw(), keep_length: false }).is_none());
    }

    #[test]
    fn clips_are_remembered_follow_their_part_and_make_room() {
        let s = triangle();
        let (s, _) = done(&s, Some(Span::new(0.0, 1.0)), Edit::Remember { name: "rise".into() });
        let (s, _) = done(&s, Some(Span::new(2.0, 4.0)), Edit::Remember { name: "fall".into() });
        assert_eq!(s.clips().len(), 2);
        // Remembered again: only renamed.
        let (s, _) = done(&s, Some(Span::new(0.0, 1.0)), Edit::Remember { name: "up".into() });
        assert_eq!(s.clips().len(), 2);
        assert_eq!(s.clips()[0].name, "up");
        let (s, _) = done(&s, None, Edit::Pin { clip: 0, pinned: true });
        // The clip goes with its part; the one it lands on is cut back.
        let (m, _) = done(&s, Some(Span::new(0.0, 1.0)), Edit::Move { to: 1.5, copy: false });
        let spans: Vec<(f64, f64)> = m.clips().iter().map(|c| (c.span.start, c.span.end)).collect();
        assert_eq!(spans, [(1.5, 2.5), (2.5, 4.0)]);
        assert!(m.clips()[0].pinned);
        // A copy: both stay, the twin isn't pinned.
        let (c, _) = done(&s, Some(Span::new(0.0, 1.0)), Edit::Move { to: 1.0, copy: true });
        assert_eq!(c.clips().len(), 3);
        assert!(c.clips()[0].pinned && !c.clips()[1].pinned);
        // A new clip over the middle of another keeps that one's earlier side.
        let (o, _) = done(&s, Some(Span::new(2.5, 3.0)), Edit::Remember { name: "mid".into() });
        let spans: Vec<(f64, f64)> = o.clips().iter().map(|c| (c.span.start, c.span.end)).collect();
        assert_eq!(spans, [(0.0, 1.0), (2.0, 2.5), (2.5, 3.0)]);
        let (f, _) = done(&s, None, Edit::Forget(0));
        assert_eq!(f.clips().len(), 1);
        assert_eq!(f.points(), s.points());
    }

    #[test]
    fn the_end_mark_cuts_or_holds() {
        let mut s = triangle();
        s.clips = vec![Clip { span: Span::new(2.0, 4.0), source: None, name: "fall".into(), pinned: false }];
        let (short, _) = done(&s, None, Edit::SetLength(3.0));
        assert!(near(short.length(), 3.0));
        assert!(near(short.value_before(3.0), 0.5));
        assert_eq!(short.clips()[0].span, Span::new(2.0, 3.0));
        let (long, _) = done(&s, None, Edit::SetLength(8.0));
        assert!(near(long.value_at(6.0), 0.0));
        assert!(near(done(&s, None, Edit::SetLength(9999.0)).0.length(), MAX_BEATS));
    }

    #[test]
    fn the_grid_snaps_unless_free_or_off() {
        let g = Grid::default();
        assert!(near(g.snap(1.1, false), 1.0));
        assert!(near(g.snap(1.13, false), 1.25));
        assert!(near(g.snap(1.13, true), 1.13));
        let t = Grid { step: Some(Step::Eighth), triplet: true, dotted: false };
        assert!(near(t.snap(0.3, false), 1.0 / 3.0));
        assert!(near(Grid { step: None, triplet: false, dotted: false }.snap(1.13, false), 1.13));
        // A dotted 1/8 is three quarters of a beat.
        let d = Grid { step: Some(Step::Eighth), triplet: false, dotted: true };
        assert!(near(d.snap(1.4, false), 1.5) && near(d.snap(1.0, false), 0.75));
    }

    #[test]
    fn a_drag_is_one_step_of_undo() {
        let mut e = Editor::new(triangle());
        assert!(!e.changed());
        for value in [0.9, 0.7, 0.5] {
            assert!(e.drag(&Edit::MovePoint { point: 1, beat: 2.0, value }));
        }
        // Each move starts from the curve as the drag began, not from the last move.
        assert!(e.drag(&Edit::MovePoint { point: 1, beat: 3.0, value: 0.5 }));
        assert!(e.dragging());
        assert!(e.let_go());
        assert!(e.changed() && e.can_undo() && !e.can_redo());
        assert!(near(e.shape().points()[1].beat, 3.0));
        assert!(e.undo());
        assert_eq!(*e.shape(), triangle());
        assert!(!e.changed() && !e.can_undo());
        assert!(e.redo());
        assert!(near(e.shape().points()[1].beat, 3.0));
        assert!(!e.redo());
    }

    #[test]
    fn a_drag_given_up_or_that_changes_nothing_is_no_step() {
        let mut e = Editor::new(triangle());
        assert!(e.drag(&Edit::MovePoint { point: 1, beat: 2.0, value: 0.2 }));
        assert!(e.give_up());
        assert_eq!(*e.shape(), triangle());
        assert!(!e.can_undo());
        // A drag that ends where it began.
        e.drag(&Edit::MovePoint { point: 1, beat: 2.0, value: 0.2 });
        assert!(!e.drag(&Edit::MovePoint { point: 1, beat: 2.0, value: 1.0 }), "back where it was: nothing to do");
        assert!(!e.let_go());
        assert!(!e.can_undo());
        // An edit that can't be done shows the curve as the drag began.
        e.drag(&Edit::MovePoint { point: 1, beat: 2.0, value: 0.2 });
        assert!(!e.drag(&Edit::MovePoint { point: 9, beat: 2.0, value: 0.2 }));
        assert_eq!(*e.shape(), triangle());
    }

    #[test]
    fn undo_brings_the_highlight_back_and_a_new_edit_drops_redo() {
        let mut e = Editor::new(triangle());
        e.set_highlight(Some(Span::new(0.0, 1.0)));
        assert!(e.edit(&Edit::Move { to: 3.0, copy: false }));
        assert_eq!(e.highlight(), Some(Span::new(3.0, 4.0)));
        assert!(e.undo());
        assert_eq!(e.highlight(), Some(Span::new(0.0, 1.0)));
        assert!(e.edit(&Edit::Flip));
        assert!(!e.can_redo());
        // A highlight too short, or outside the curve, is none.
        e.set_highlight(Some(Span::new(1.0, 1.001)));
        assert_eq!(e.highlight(), None);
        e.set_highlight(Some(Span::new(3.0, 9.0)));
        assert_eq!(e.highlight(), Some(Span::new(3.0, 4.0)));
    }

    #[test]
    fn changed_follows_the_saved_curve_through_undo() {
        let mut e = Editor::new(triangle());
        e.edit(&Edit::Flip);
        e.saved();
        assert!(!e.changed());
        e.undo();
        assert!(e.changed(), "undone past the save");
        e.redo();
        assert!(!e.changed());
        e.undo();
        e.edit(&Edit::Reverse);
        assert!(e.changed());
        e.undo();
        assert!(e.changed(), "the saved curve was dropped with redo: it can't be got back to");
    }

    #[test]
    fn undo_keeps_only_so_many_steps() {
        let mut e = Editor::new(ramp());
        for i in 0..UNDO_STEPS + 20 {
            assert!(e.edit(&Edit::MovePoint { point: 1, beat: 4.0, value: (i % 7) as f64 / 10.0 }));
        }
        let mut steps = 0;
        while e.undo() {
            steps += 1;
        }
        assert_eq!(steps, UNDO_STEPS);
        assert!(e.changed(), "the opened curve fell off the end");
    }

    /// A gate over 4 beats: 0, then 1 from beat 2.
    fn gate() -> Shape {
        Shape::new(vec![pt(0.0, 0.0), pt(2.0, 0.0), pt(2.0, 1.0), pt(4.0, 1.0)], 4.0)
    }

    #[test]
    fn repeats_on_a_triplet_grid_keep_every_tooth() {
        // Thirds of a beat don't add up exactly: each tooth's end and the next one's start are the same beat but for
        // the sum's last digit, and must stay in that order.
        let third = 1.0 / 3.0;
        let s = Shape::new(vec![pt(0.0, 0.0), pt(third, 1.0), pt(third, 0.0), pt(8.0, 0.0)], 8.0);
        let (r, _) = done(&s, Some(Span::new(0.0, third)), Edit::Repeat { start: 0.0, end: 8.0 });
        for tooth in 0..24 {
            let start = f64::from(tooth) * third;
            let (early, late) = (r.value_at(start + third * 0.25), r.value_at(start + third * 0.75));
            assert!((early - 0.25).abs() < 1e-6 && (late - 0.75).abs() < 1e-6, "tooth {tooth}: {early}, {late}");
        }
    }

    #[test]
    fn a_jump_that_ends_the_points_still_holds_its_value() {
        // The last point goes: the curve holds 1 from the jump on, not 0.
        let (r, _) = done(&gate(), None, Edit::RemovePoint(3));
        assert!(near(r.value_at(1.0), 0.0) && near(r.value_at(3.0), 1.0));
        let (r, _) = done(&gate(), None, Edit::RemovePoint(0));
        assert!(near(r.value_at(1.0), 0.0) && near(r.value_at(3.0), 1.0));
        // An edit inside a held tail leaves the hold after it.
        let (long, _) = done(&triangle(), None, Edit::SetLength(8.0));
        let (s, _) = done(&long, None, Edit::MovePoint { point: 2, beat: 4.0, value: 0.4 });
        let (t, _) = done(&s, Some(Span::new(5.0, 6.0)), Edit::Swap { piece: saw(), keep_length: false });
        assert!(near(t.value_at(5.5), 0.5));
        assert!(near(t.value_at(7.0), 0.4), "after the part the tail holds what it held: {}", t.value_at(7.0));
        assert!(near(t.value_at(4.5), 0.4));
    }

    #[test]
    fn an_edit_that_cant_be_done_leaves_nothing_in_hand() {
        let mut e = Editor::new(triangle());
        assert!(!e.edit(&Edit::RemoveTime));
        assert!(!e.dragging());
        e.set_highlight(Some(Span::new(0.0, 1.0)));
        assert!(e.drag(&Edit::Move { to: 2.0, copy: false }));
        assert_eq!(e.highlight(), Some(Span::new(2.0, 3.0)));
    }

    #[test]
    fn a_dropped_curve_holds_its_first_and_last_value() {
        // Its points don't reach its edges: the list draws it held there, and so does the editor.
        let piece = Piece {
            points: vec![CurvePoint { x: 0.25, y: 1.0, bend: 0.0 }, CurvePoint { x: 0.75, y: 0.0, bend: 0.0 }],
            ..saw()
        };
        let (i, _) = done(&gate(), None, Edit::Insert { at: 1.0, piece: piece.clone() });
        assert!(near(i.value_at(1.25), 1.0) && near(i.value_at(2.75), 0.0), "{}", i.value_at(1.25));
        let (f, _) = done(&gate(), Some(Span::new(0.0, 2.0)), Edit::Swap { piece, keep_length: false });
        assert!(near(f.value_at(0.25), 1.0) && near(f.value_at(1.75), 0.0));
    }

    #[test]
    fn a_handle_let_go_where_it_was_is_no_edit() {
        let s = Shape::new(vec![Point::new(0.0, 0.0, 0.7), pt(4.0, 1.0)], 4.0);
        let lit = Some(Span::new(1.0, 2.0));
        let (low, high) = s.bounds(Span::new(1.0, 2.0));
        for edit in [
            Edit::Move { to: 1.0, copy: false },
            Edit::Move { to: 1.0, copy: true },
            Edit::Speed { start: 1.0, end: 2.0 },
            Edit::Repeat { start: 1.0, end: 2.0 },
            Edit::Squash { low, high },
            Edit::Tilt { left: 1.0, right: 1.0, from_top: false },
        ] {
            assert!(apply(&s, lit, &edit).is_none(), "{edit:?}");
        }
        // A pinned clip copied onto itself keeps its pin.
        let mut e = Editor::new(s);
        e.set_highlight(lit);
        e.edit(&Edit::Remember { name: "part".into() });
        e.edit(&Edit::Pin { clip: 0, pinned: true });
        assert!(!e.edit(&Edit::Move { to: 1.0, copy: true }));
        assert!(e.shape().clips()[0].pinned);
    }

    #[test]
    fn a_swap_over_all_but_a_sliver_leaves_only_the_new_curve() {
        let mut s = triangle();
        s.clips = vec![Clip { span: Span::new(0.0, 4.0), source: None, name: "all".into(), pinned: false }];
        let (own, lit) = done(&s, Some(Span::new(0.01, 4.0)), Edit::Swap { piece: saw(), keep_length: true });
        assert!(near(own.length(), 2.0));
        assert_eq!(lit, Some(Span::new(0.0, 2.0)));
        assert!(near(own.value_at(1.0), 0.5));
        assert_eq!(own.clips().len(), 1);
        assert_eq!(own.clips()[0].span, Span::new(0.0, 2.0));
    }

    #[test]
    fn a_highlight_thats_not_a_number_is_no_highlight() {
        let broken = Span { start: f64::NAN, end: 1.0 };
        let (f, lit) = done(&triangle(), Some(broken), Edit::Flip);
        assert_eq!(lit, None);
        assert!(near(f.value_at(2.0), 0.0), "the whole curve flipped");
        assert_eq!(triangle().bounds(broken), (0.0, 0.0));
        let mut e = Editor::new(triangle());
        e.set_highlight(Some(broken));
        assert_eq!(e.highlight(), None);
        assert!(apply(&triangle(), None, &Edit::Speed { start: f64::NAN, end: 1.0 }).is_none());
    }

    #[test]
    fn a_shallow_part_of_a_hard_bend_stretched_tall_keeps_its_picture() {
        let s = Shape::new(vec![Point::new(0.0, 0.0, -1.0), pt(4.0, 1.0)], 4.0);
        let span = Span::new(2.0, 3.0);
        let (low, high) = s.bounds(span);
        let (q, _) = done(&s, Some(span), Edit::Squash { low: 0.0, high: 1.0 });
        for i in 1..100 {
            let beat = 2.0 + f64::from(i) / 100.0;
            let true_value = (s.value_at(beat) - low) / (high - low);
            assert!((q.value_at(beat) - true_value).abs() < 0.015, "at {beat}: {} for {true_value}", q.value_at(beat));
        }
    }

    #[test]
    fn smooth_walks_a_long_stepped_curve_once() {
        // 256 beats of 1/16 steps: 8,192 points. Quick enough to follow the mouse.
        let (steps, _) = done(&Shape::new(vec![pt(0.0, 0.0), pt(256.0, 1.0)], 256.0), None, Edit::Steps { step: 0.25 });
        assert!(steps.points().len() > 2000);
        let began = std::time::Instant::now();
        let (smooth, _) = done(&steps, None, Edit::Smooth { amount: 0.5, short: 0.25 });
        assert!(began.elapsed() < std::time::Duration::from_secs(2), "{:?}", began.elapsed());
        for pair in smooth.points().windows(2) {
            assert!(pair[1].value >= pair[0].value - 1e-9, "it only rises");
        }
    }

    #[test]
    fn no_edit_leaves_the_curve_out_of_order_or_out_of_range() {
        let s = Shape::new(
            vec![
                Point::new(0.0, 0.1, 0.6),
                pt(1.0, 0.8),
                pt(1.0, 0.3),
                Point::new(2.5, 0.5, -0.4),
                pt(3.0, 0.0),
                pt(3.0, 1.0),
                pt(4.0, 1.0),
            ],
            4.0,
        );
        let edits = [
            Edit::Move { to: 2.2, copy: false },
            Edit::Move { to: 0.0, copy: true },
            Edit::Speed { start: 0.3, end: 3.9 },
            Edit::Speed { start: 3.0, end: 0.5 },
            Edit::Repeat { start: 0.75, end: 4.0 },
            Edit::Repeat { start: 0.75, end: 1.0 },
            Edit::Repeat { start: 2.0, end: 2.25 },
            Edit::Squash { low: 0.9, high: 0.1 },
            Edit::Tilt { left: 0.0, right: 1.0, from_top: false },
            Edit::Tilt { left: 1.0, right: 0.3, from_top: true },
            Edit::Smooth { amount: 1.0, short: 0.25 },
            Edit::Smooth { amount: 0.3, short: 0.25 },
            Edit::Smooth { amount: -1.0, short: 0.25 },
            Edit::Flip,
            Edit::Reverse,
            Edit::Steps { step: 1.0 / 3.0 },
            Edit::Flatten,
            Edit::RemoveTime,
            Edit::Insert { at: 1.0, piece: saw() },
            Edit::Swap { piece: saw(), keep_length: false },
            Edit::Swap { piece: saw(), keep_length: true },
            Edit::SetLength(1.7),
        ];
        for lit in [Span::new(0.75, 2.25), Span::new(0.0, 1.0), Span::new(1.0, 3.0), Span::new(2.9, 4.0)] {
            for edit in &edits {
                let Some((after, lit_after)) = apply(&s, Some(lit), edit) else { continue };
                let what = format!("{edit:?} on {lit:?}");
                assert!(after.length() >= SHORTEST && after.length() <= MAX_BEATS, "{what}");
                for pair in after.points().windows(2) {
                    assert!(pair[0].beat <= pair[1].beat, "{what}: out of order");
                }
                for (i, p) in after.points().iter().enumerate() {
                    assert!((0.0..=after.length() + 1e-9).contains(&p.beat), "{what}: a point outside the curve");
                    assert!((0.0..=1.0).contains(&p.value) && (-1.0..=1.0).contains(&p.bend), "{what}: {p:?}");
                    let at_beat = after.points().iter().filter(|q| same(q.beat, p.beat)).count();
                    assert!(at_beat <= 2, "{what}: {at_beat} points at beat {} (point {i})", p.beat);
                }
                if let Some(l) = lit_after {
                    assert!(l.start >= 0.0 && l.end <= after.length() + 1e-9 && !l.is_empty(), "{what}: {l:?}");
                }
                for pair in after.clips().windows(2) {
                    assert!(pair[0].span.end <= pair[1].span.start + 1e-9, "{what}: clips overlap");
                }
                // Outside the stretches the edit touches, the curve is as it was (edits that move time aside).
                let moves_time = matches!(edit, Edit::RemoveTime | Edit::Insert { .. } | Edit::SetLength(_))
                    || matches!(edit, Edit::Swap { keep_length: true, .. });
                if !moves_time {
                    let touched = lit_after.map_or(lit, |l| Span::new(lit.start.min(l.start), lit.end.max(l.end)));
                    for i in 0..200 {
                        let beat = (i as f64 + 0.41) / 200.0 * 4.0;
                        if beat < touched.start - 1e-6 || beat > touched.end + 1e-6 {
                            let (x, y) = (s.value_at(beat), after.value_at(beat));
                            assert!((x - y).abs() < 0.012, "{what}: changed at beat {beat} outside it: {x} to {y}");
                        }
                    }
                }
            }
        }
    }
}
