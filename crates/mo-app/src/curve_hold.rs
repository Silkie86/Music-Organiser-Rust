//! The highlight on the curve editor's picture and its handles, worked out with no screen (the screen design of
//! 14 Nov 2026, "The highlight"): the frame round the highlighted part, which handle the mouse has, the edit a drag
//! of it makes ([`crate::curve_edit::Edit`]), the read-out's words and the hint line. A highlight is a stretch of
//! time: its bar moves it (Alt smooths or sharpens, Ctrl copies), its sides change its speed (through the other side:
//! backwards; Ctrl repeats), its top and bottom squash it (through the other: upside down), its corners fade it.
//! With nothing highlighted the sides, top and bottom stand round the whole curve.

use crate::curve_edit::{Edit, Grid, Shape, Span};
use crate::curve_picture::{Frame, TOP};
use crate::curve_touch::{self, Hint, Keys, Under, percent, place_words};

/// How far outside the frame a handle is still grabbed (its grab area is 24 across), and how far inside: a dot on
/// the frame's edge is reached by coming at it from inside.
pub const REACH: f64 = 12.0;
const REACH_IN: f64 = 4.0;
/// With nothing highlighted, a handle is grabbed on its pill only: this far along it from its middle.
const PILL_REACH: f64 = 17.0;
/// How far an Alt-drag of the bar goes to smooth or sharpen all the way, in points.
const SMOOTH_REACH: f64 = 80.0;
/// Shift makes the top, the bottom and the corners this much finer.
const FINE: f64 = 0.25;

/// A handle of the highlight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Bar,
    Left,
    Right,
    Top,
    Bottom,
    /// A corner: on the left (else the right), at the top (else the bottom).
    Corner {
        left: bool,
        top: bool,
    },
}

/// The frame round the highlighted part (or the whole curve), in points from the picture's left and top: its time
/// across, its lowest value to its highest up and down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
    /// Nothing is highlighted: it's round the whole curve.
    pub whole: bool,
}

/// The frame of the highlight, or of the whole curve when nothing is highlighted. It may reach out of sight.
pub fn frame_of(shape: &Shape, highlight: Option<Span>, frame: Frame) -> Option<Box> {
    if !frame.fits() {
        return None;
    }
    let span = highlight.unwrap_or(shape.whole());
    let (low, high) = shape.bounds(span);
    Some(Box {
        left: frame.x(span.start),
        right: frame.x(span.end),
        top: frame.y(high),
        bottom: frame.y(low),
        whole: highlight.is_none(),
    })
}

/// What's under the mouse, the handles too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Over {
    Part(Part),
    Dot(usize),
    Line(usize),
    /// Inside the highlight's time, on nothing.
    Inside,
    Empty,
}

/// The handle of `b` at `x`, `y`, if any. The bar has the room above the picture's top line; the other handles keep
/// out of it.
fn part_at(b: Box, x: f64, y: f64) -> Option<Part> {
    let (mid_x, mid_y) = ((b.left + b.right) / 2.0, (b.top + b.bottom) / 2.0);
    if b.whole {
        // On the pills only: the rest of the picture's edge is the dots' and the lines'.
        let along_side = (y - mid_y).abs() <= PILL_REACH;
        let along_top = (x - mid_x).abs() <= PILL_REACH;
        return if along_side && (x - b.left).abs() <= REACH {
            Some(Part::Left)
        } else if along_side && (x - b.right).abs() <= REACH {
            Some(Part::Right)
        } else if along_top && (y - b.top).abs() <= REACH && y >= TOP - REACH_IN {
            Some(Part::Top)
        } else if along_top && (y - b.bottom).abs() <= REACH {
            Some(Part::Bottom)
        } else {
            None
        };
    }
    let (wide, tall) = (b.right - b.left, b.bottom - b.top);
    let (in_x, in_y) = (REACH_IN.min(wide / 4.0), REACH_IN.min(tall / 4.0));
    // The room above the picture's top line is the bar's.
    let roof = (b.top - REACH).max(TOP);
    let at_left = (b.left - REACH..=b.left + in_x).contains(&x);
    let at_right = (b.right - in_x..=b.right + REACH).contains(&x);
    let at_top = (roof..=b.top + in_y).contains(&y);
    let at_bottom = (b.bottom - in_y..=b.bottom + REACH).contains(&y);
    let between_x = x > b.left && x < b.right;
    let between_y = y >= roof && y <= b.bottom + REACH;
    // A part with no height to speak of has nothing to fade or squash: its sides come first.
    if tall < 2.0 * REACH && between_y && (at_left || at_right) {
        return Some(if at_left { Part::Left } else { Part::Right });
    }
    match (at_left, at_right, at_top, at_bottom) {
        (true, _, true, _) => Some(Part::Corner { left: true, top: true }),
        (_, true, true, _) => Some(Part::Corner { left: false, top: true }),
        (true, _, _, true) => Some(Part::Corner { left: true, top: false }),
        (_, true, _, true) => Some(Part::Corner { left: false, top: false }),
        (true, ..) if between_y => Some(Part::Left),
        (_, true, ..) if between_y => Some(Part::Right),
        (_, _, true, _) if between_x => Some(Part::Top),
        (_, _, _, true) if between_x => Some(Part::Bottom),
        _ if y < TOP && x >= b.left && x <= b.right => Some(Part::Bar),
        _ => None,
    }
}

/// What's under the mouse at `x`, `y`. What wins, first to last: a corner, a side or the top or bottom, a dot, the
/// bar, a line, the highlight's inside. With nothing highlighted the pills round the whole curve come after the
/// dots and the lines, so the curve under a pill can still be dragged and given points.
pub fn over(shape: &Shape, highlight: Option<Span>, frame: Frame, x: f64, y: f64) -> Over {
    over_in(shape, frame_of(shape, highlight, frame), frame, x, y)
}

/// [`over`] with the highlight's frame already worked out ([`frame_of`]).
pub fn over_in(shape: &Shape, b: Option<Box>, frame: Frame, x: f64, y: f64) -> Over {
    if !frame.fits() || !x.is_finite() || !y.is_finite() {
        return Over::Empty;
    }
    let whole = b.is_none_or(|b| b.whole);
    let part = b.and_then(|b| part_at(b, x, y));
    let under = curve_touch::under(shape, frame, x, y);
    match (part, under) {
        (Some(Part::Bar), Under::Dot(point)) => Over::Dot(point),
        (Some(_), Under::Dot(point)) if whole => Over::Dot(point),
        (Some(_), Under::Line(line)) if whole => Over::Line(line),
        (Some(part), _) => Over::Part(part),
        (None, Under::Dot(point)) => Over::Dot(point),
        (None, Under::Line(line)) => Over::Line(line),
        (None, Under::Empty) => {
            let inside = b.is_some_and(|b| !b.whole && x > b.left && x < b.right);
            if inside { Over::Inside } else { Over::Empty }
        }
    }
}

/// A handle in hand, with what a drag needs of how things were at the press: the highlight then, the lowest and
/// highest value in it, and where the press was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hold {
    pub part: Part,
    pub span: Span,
    pub low: f64,
    pub high: f64,
    pub x: f64,
    pub y: f64,
}

/// Takes hold of `part` at `x`, `y`. With nothing highlighted the whole curve is the part held: the caller highlights
/// [`Hold::span`] before the drag's first edit, as the sides need a highlight.
pub fn hold(shape: &Shape, highlight: Option<Span>, part: Part, x: f64, y: f64) -> Hold {
    let span = highlight.unwrap_or(shape.whole());
    let (low, high) = shape.bounds(span);
    Hold { part, span, low, high, x, y }
}

/// The edit a drag of a handle makes with the mouse now at `x`, `y`, and the read-out's words for it. The edit is
/// made on the curve as it was when the drag began, `length` beats long.
pub fn drag(hold: Hold, length: f64, frame: Frame, grid: Grid, keys: Keys, x: f64, y: f64) -> (Edit, String) {
    let Hold { span, low, high, .. } = hold;
    let beats = frame.beat_at(x) - frame.beat_at(hold.x);
    let fine = if keys.shift { FINE } else { 1.0 };
    let values = (frame.value_at(y) - frame.value_at(hold.y)) * fine;
    // A level part has no height: it moves as it is, and a fade is counted against the picture's height.
    let level = high - low < 1e-6;
    let tall = if level { 1.0 } else { high - low };
    match hold.part {
        Part::Bar if keys.alt => {
            let amount = ((hold.y - y) / SMOOTH_REACH).clamp(-1.0, 1.0);
            let short = grid.beats().unwrap_or(0.25);
            (Edit::Smooth { amount, short }, smooth_words(amount))
        }
        Part::Bar => {
            let to = grid.snap(span.start + beats, keys.shift);
            let lands = to.clamp(0.0, (length - span.len()).max(0.0));
            let words = format!("{}{}", if keys.ctrl { "copy to " } else { "" }, place_words(lands));
            (Edit::Move { to, copy: keys.ctrl }, words)
        }
        Part::Left | Part::Right => {
            let left = hold.part == Part::Left;
            let edge = if left { span.start } else { span.end };
            let edge = grid.snap(edge + beats, keys.shift).clamp(0.0, length.max(0.0));
            // A repeat has no backwards: its side stops at the other one.
            let edge = match (keys.ctrl, left) {
                (true, true) => edge.min(span.end),
                (true, false) => edge.max(span.start),
                _ => edge,
            };
            let (start, end) = if left { (edge, span.end) } else { (span.start, edge) };
            let now = (end - start).abs();
            if now < 1e-9 {
                // On the other side itself there's nothing left of it: the curve shows as it was.
                let edit = if keys.ctrl { Edit::Repeat { start, end } } else { Edit::Speed { start, end } };
                (edit, "nothing left".to_owned())
            } else if keys.ctrl {
                (Edit::Repeat { start, end }, repeat_words(span.len(), now))
            } else {
                (Edit::Speed { start, end }, speed_words(span.len(), now, start > end))
            }
        }
        Part::Top | Part::Bottom if level => {
            let to = (high + values).clamp(0.0, 1.0);
            (Edit::Squash { low: to, high: to }, format!("level at {}", percent(to)))
        }
        Part::Top => {
            let to = (high + values).clamp(0.0, 1.0);
            let flipped = if to < low { "flipped · " } else { "" };
            let words = format!("{flipped}height {}", percent((to - low).abs() / tall));
            (Edit::Squash { low, high: to }, words)
        }
        Part::Bottom => {
            let to = (low + values).clamp(0.0, 1.0);
            let flipped = if to > high { "flipped · " } else { "" };
            (Edit::Squash { low: to, high }, format!("{flipped}floor {}", percent(to)))
        }
        Part::Corner { left, top } => {
            // A top corner pulled down, or a bottom one pushed up, fades that end; the other way puts it back.
            let faded = (if top { -values } else { values } / tall).clamp(0.0, 1.0);
            let kept = 1.0 - faded;
            let edit = if left {
                Edit::Tilt { left: kept, right: 1.0, from_top: !top }
            } else {
                Edit::Tilt { left: 1.0, right: kept, from_top: !top }
            };
            (edit, format!("fade {} {}", if left { "in" } else { "out" }, percent(faded)))
        }
    }
}

/// A new highlight dragged out over empty space, from the press at `from` to the mouse at `x` (points from the
/// picture's left): on the grid unless Shift is held, within the curve.
pub fn swept(frame: Frame, grid: Grid, keys: Keys, length: f64, from: f64, x: f64) -> Span {
    let at = |x: f64| grid.snap(frame.beat_at(x), keys.shift).clamp(0.0, length.max(0.0));
    Span::new(at(from), at(x))
}

/// The read-out's words for a highlight being dragged out: where it starts and how long it is.
pub fn swept_words(span: Span) -> String {
    format!("{} · {}", place_words(span.start), length_words(span.len()))
}

/// The hint line's words for a handle. `mac`: the keys are named as a Mac's are.
pub fn hint(over: Over, mac: bool) -> Hint {
    let key = |pc: &'static str, on_mac: &'static str| if mac { on_mac } else { pc };
    match over {
        Over::Part(Part::Bar) => Hint {
            name: "Highlight",
            does: "drag to move it",
            keys: key(
                "Alt: smooth or sharpen, up smooths, down sharpens · Ctrl: copy the part",
                "Option: smooth or sharpen, up smooths, down sharpens · Cmd: copy the part",
            ),
        },
        Over::Part(Part::Left | Part::Right) => Hint {
            name: "Side",
            does: "drag changes speed · past the other side reverses",
            keys: key(
                "Ctrl: repeat the part, drag out to repeat it, in for a stutter · Shift: off the grid",
                "Cmd: repeat the part, drag out to repeat it, in for a stutter · Shift: off the grid",
            ),
        },
        Over::Part(Part::Top) => {
            Hint { name: "Top", does: "drag down squashes · past the bottom flips", keys: "Shift: finely" }
        }
        Over::Part(Part::Bottom) => {
            Hint { name: "Bottom", does: "drag up squashes · past the top flips", keys: "Shift: finely" }
        }
        Over::Part(Part::Corner { left, .. }) => Hint {
            name: "Corner",
            does: if left { "drag to fade in" } else { "drag to fade out" },
            keys: "Shift: finely",
        },
        Over::Dot(point) => curve_touch::hint(Under::Dot(point), mac),
        Over::Line(line) => curve_touch::hint(Under::Line(line), mac),
        Over::Inside => Hint {
            name: "Highlight",
            does: "drag its bar, sides and corners · Esc drops it",
            keys: "Shift: off the grid",
        },
        Over::Empty => {
            Hint { name: "Empty", does: "drag to highlight · double-click adds a point", keys: "Shift: off the grid" }
        }
    }
}

/// A number with at most two decimals, none when it's whole ("2", "1.5", "0.67").
fn short(n: f64) -> String {
    let s = format!("{n:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A length in beats as words: "1 bar", "2 bars", "1 beat", "1/2 beat", "1.5 beats".
fn length_words(beats: f64) -> String {
    let bars = beats / crate::curve_picture::BAR;
    if bars >= 1.0 && (bars - bars.round()).abs() < 1e-6 {
        return format!("{} bar{}", bars.round() as u64, if bars.round() == 1.0 { "" } else { "s" });
    }
    for (part, name) in [(1.0, "1 beat"), (0.5, "1/2 beat"), (0.25, "1/4 beat"), (0.125, "1/8 beat")] {
        if (beats - part).abs() < 1e-6 {
            return name.to_owned();
        }
    }
    format!("{} beats", short(beats))
}

/// A change of speed in words: a part `was` beats long now plays over `now` beats.
fn speed_words(was: f64, now: f64, backwards: bool) -> String {
    let times = was / now.max(1e-9);
    let speed = if (times - 1.0).abs() < 1e-6 {
        "same speed".to_owned()
    } else if (times - 2.0).abs() < 1e-6 {
        "double speed".to_owned()
    } else if (times - 0.5).abs() < 1e-6 {
        "half speed".to_owned()
    } else {
        format!("×{} speed", short(times))
    };
    if backwards { format!("reversed · {speed}") } else { speed }
}

/// A repeat in words: a part `was` beats long dragged to `now` beats. Dragged out it repeats that many times;
/// dragged in, what's left repeats to fill it (a stutter), and its length is said too.
fn repeat_words(was: f64, now: f64) -> String {
    if now > was + 1e-9 {
        format!("repeat ×{}", short(now / was.max(1e-9)))
    } else {
        format!("repeat ×{} · {}", short(was / now.max(1e-9)), length_words(now))
    }
}

/// Smoothing or sharpening in words.
fn smooth_words(amount: f64) -> String {
    let part = percent(amount.abs());
    if amount > 0.0 {
        format!("smoother {part}")
    } else if amount < 0.0 {
        format!("sharper {part}")
    } else {
        "as it is".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve_edit::{Step, apply};
    use crate::curve_picture::{SIDE, View};
    use mo_formats::bitwig::curve::CurvePoint;

    /// Two bars: a ramp up over the first bar to 0.8, then level at 0.2.
    fn shape() -> Shape {
        let p = |x, y, bend| CurvePoint { x, y, bend };
        Shape::from_curve(&[p(0.0, 0.0, 0.0), p(0.5, 0.8, 0.0), p(0.5, 0.2, 0.0), p(1.0, 0.2, 0.0)], 8.0)
    }

    /// 50 points a beat and 200 points of height.
    fn frame() -> Frame {
        Frame { width: 400.0 + 2.0 * SIDE, height: 200.0 + 2.0 * TOP, view: View::whole(8.0) }
    }

    fn at(beat: f64, value: f64) -> (f64, f64) {
        (frame().x(beat), frame().y(value))
    }

    const NO_KEYS: Keys = Keys { shift: false, alt: false, ctrl: false };
    const SHIFT: Keys = Keys { shift: true, alt: false, ctrl: false };
    const ALT: Keys = Keys { shift: false, alt: true, ctrl: false };
    const CTRL: Keys = Keys { shift: false, alt: false, ctrl: true };
    const GRID: Grid = Grid { step: Some(Step::Quarter), triplet: false, dotted: false };

    /// Beats 1 to 3 of the ramp: from 0.2 up to 0.6.
    fn lit() -> Option<Span> {
        Some(Span::new(1.0, 3.0))
    }

    #[test]
    fn the_frame_is_the_parts_time_and_its_lowest_to_highest() {
        let (s, f) = (shape(), frame());
        let b = frame_of(&s, lit(), f).unwrap();
        let ((l, bottom), (r, top)) = (at(1.0, 0.2), at(3.0, 0.6));
        assert!((b.left - l).abs() < 1e-6 && (b.right - r).abs() < 1e-6);
        assert!((b.top - top).abs() < 1e-6 && (b.bottom - bottom).abs() < 1e-6);
        assert!(!b.whole);
        // Nothing highlighted: round the whole curve.
        let b = frame_of(&s, None, f).unwrap();
        assert!(b.whole);
        assert!((b.left - SIDE).abs() < 1e-6 && (b.top - at(0.0, 0.8).1).abs() < 1e-6);
    }

    #[test]
    fn handles_win_at_the_edge_and_dots_from_inside() {
        let (s, f) = (shape(), frame());
        let over_at = |beat: f64, value: f64, dx: f64, dy: f64| {
            let (x, y) = at(beat, value);
            over(&s, lit(), f, x + dx, y + dy)
        };
        assert_eq!(over_at(1.0, 0.2, 0.0, 0.0), Over::Part(Part::Corner { left: true, top: false }));
        assert_eq!(over_at(3.0, 0.6, 2.0, -2.0), Over::Part(Part::Corner { left: false, top: true }));
        assert_eq!(over_at(1.0, 0.4, -5.0, 0.0), Over::Part(Part::Left));
        assert_eq!(over_at(3.0, 0.4, 8.0, 0.0), Over::Part(Part::Right));
        assert_eq!(over_at(2.0, 0.6, 0.0, -8.0), Over::Part(Part::Top));
        assert_eq!(over_at(2.0, 0.2, 0.0, 8.0), Over::Part(Part::Bottom));
        // The bar is in the room above the picture's top line, over the highlight's time only.
        assert_eq!(over(&s, lit(), f, at(2.0, 0.0).0, 6.0), Over::Part(Part::Bar));
        assert_eq!(over(&s, lit(), f, at(5.0, 0.0).0, 6.0), Over::Empty);
        // Inside, the line; off the line, the inside; outside, nothing.
        assert_eq!(over_at(2.0, 0.4, 0.0, 0.0), Over::Line(0));
        assert_eq!(over_at(2.0, 0.3, 10.0, 0.0), Over::Inside);
        assert_eq!(over_at(6.0, 0.6, 0.0, 0.0), Over::Empty);
        // A dot on the frame's edge: the handle has the edge itself, the dot is reached from inside.
        let whole_bar = Some(Span::new(0.0, 4.0));
        let (x, y) = at(4.0, 0.8);
        assert_eq!(over(&s, whole_bar, f, x, y), Over::Part(Part::Corner { left: false, top: true }));
        assert_eq!(over(&s, whole_bar, f, x - 8.0, y + 8.0), Over::Dot(1));
    }

    #[test]
    fn with_nothing_highlighted_the_pills_stand_round_the_whole_curve_and_dots_come_first() {
        let (s, f) = (shape(), frame());
        let (x, y) = at(0.0, 0.4);
        assert_eq!(over(&s, None, f, x, y), Over::Part(Part::Left));
        let (x, y) = at(8.0, 0.4);
        assert_eq!(over(&s, None, f, x, y), Over::Part(Part::Right));
        let (x, y) = at(4.0, 0.0);
        assert_eq!(over(&s, None, f, x, y + 4.0), Over::Part(Part::Bottom));
        // On the curve itself the line is held, pill or no pill: the top pill sits on the level part's line.
        let flat = Shape::from_curve(
            &[CurvePoint { x: 0.0, y: 0.5, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.5, bend: 0.0 }],
            8.0,
        );
        let (x, y) = at(4.0, 0.5);
        assert_eq!(over(&flat, None, f, x, y), Over::Line(0));
        assert_eq!(over(&flat, None, f, x, y - 9.0), Over::Part(Part::Top));
        // Off the pill the edge is nothing to hold, and there's no bar.
        let (x, y) = at(0.0, 0.75);
        assert_eq!(over(&s, None, f, x - 6.0, y), Over::Empty);
        assert_eq!(over(&s, None, f, at(4.0, 0.0).0, 6.0), Over::Empty);
        // The first point sits on the left edge: it's the dot that's grabbed.
        let (x, y) = at(0.0, 0.0);
        assert_eq!(over(&s, None, f, x, y), Over::Dot(0));
    }

    #[test]
    fn a_side_changes_speed_and_through_the_other_side_reverses() {
        let (s, f) = (shape(), frame());
        let (x, y) = at(3.0, 0.4);
        let h = hold(&s, lit(), Part::Right, x, y);
        // The right side from beat 3 to beat 2: beats 1 to 3 now play over 1 to 2.
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, at(2.0, 0.0).0, y);
        assert_eq!(edit, Edit::Speed { start: 1.0, end: 2.0 });
        assert_eq!(words, "double speed");
        let (after, lit_after) = apply(&s, lit(), &edit).unwrap();
        assert_eq!(lit_after, Some(Span::new(1.0, 2.0)));
        assert!((after.value_at(1.5) - 0.4).abs() < 1e-6);
        // Out to beat 5: half speed.
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, at(5.0, 0.0).0, y);
        assert_eq!((edit, words.as_str()), (Edit::Speed { start: 1.0, end: 5.0 }, "half speed"));
        // Through the left side to beat 0: backwards, at double speed.
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, at(0.0, 0.0).0, y);
        assert_eq!((edit, words.as_str()), (Edit::Speed { start: 1.0, end: 0.0 }, "reversed · double speed"));
        // Shift: off the grid.
        let (edit, words) = drag(h, s.length(), f, GRID, SHIFT, at(2.3, 0.0).0, y);
        assert_eq!(edit, Edit::Speed { start: 1.0, end: 2.3 });
        assert_eq!(words, "×1.54 speed");
    }

    #[test]
    fn ctrl_on_a_side_repeats_out_and_stutters_in() {
        let (s, f) = (shape(), frame());
        let (x, y) = at(3.0, 0.4);
        let h = hold(&s, lit(), Part::Right, x, y);
        let (edit, words) = drag(h, s.length(), f, GRID, CTRL, at(7.0, 0.0).0, y);
        assert_eq!((edit, words.as_str()), (Edit::Repeat { start: 1.0, end: 7.0 }, "repeat ×3"));
        // In to beat 2: the first beat of the part, twice.
        let (edit, words) = drag(h, s.length(), f, GRID, CTRL, at(1.9, 0.0).0, y);
        assert!(apply(&s, lit(), &edit).is_some());
        assert_eq!((edit, words.as_str()), (Edit::Repeat { start: 1.0, end: 2.0 }, "repeat ×2 · 1 beat"));
        // A repeat has no backwards: the side stops at the other one, and there nothing is left to repeat.
        let (edit, words) = drag(h, s.length(), f, GRID, CTRL, at(0.0, 0.0).0, y);
        assert!(apply(&s, lit(), &edit).is_none());
        assert_eq!((edit, words.as_str()), (Edit::Repeat { start: 1.0, end: 1.0 }, "nothing left"));
        // The same without Ctrl, on the other side itself.
        let (_, words) = drag(h, s.length(), f, GRID, NO_KEYS, at(1.0, 0.0).0, y);
        assert_eq!(words, "nothing left");
    }

    #[test]
    fn the_top_squashes_and_through_the_bottom_flips() {
        let (s, f) = (shape(), frame());
        let (x, y) = at(2.0, 0.6);
        let h = hold(&s, lit(), Part::Top, x, y);
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, x, at(2.0, 0.4).1);
        let Edit::Squash { low, high } = edit else { panic!("not a squash") };
        assert!((low - 0.2).abs() < 1e-6 && (high - 0.4).abs() < 1e-6);
        assert_eq!(words, "height 50%");
        // Through the bottom: upside down.
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, x, at(2.0, 0.1).1);
        let Edit::Squash { low, high } = edit else { panic!("not a squash") };
        assert!(high < low);
        assert_eq!(words, "flipped · height 25%");
        // Shift: a quarter as far.
        let (edit, _) = drag(h, s.length(), f, GRID, SHIFT, x, at(2.0, 0.2).1);
        let Edit::Squash { high, .. } = edit else { panic!("not a squash") };
        assert!((high - 0.5).abs() < 1e-6);
        // A level part moves as it is, and follows the mouse.
        let level = Some(Span::new(5.0, 7.0));
        let (x, y) = at(6.0, 0.2);
        let h = hold(&s, level, Part::Top, x, y);
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, x, at(6.0, 0.5).1);
        assert_eq!(words, "level at 50%");
        let (after, _) = apply(&s, level, &edit).unwrap();
        assert!((after.value_at(6.0) - 0.5).abs() < 1e-6);
        // The bottom says where the floor is.
        let (x, y) = at(2.0, 0.2);
        let h = hold(&s, lit(), Part::Bottom, x, y);
        let (_, words) = drag(h, s.length(), f, GRID, NO_KEYS, x, at(2.0, 0.3).1);
        assert_eq!(words, "floor 30%");
    }

    #[test]
    fn a_corner_fades_its_end() {
        let (s, f) = (shape(), frame());
        let (x, y) = at(3.0, 0.6);
        let h = hold(&s, lit(), Part::Corner { left: false, top: true }, x, y);
        // The top right corner down a quarter of the part's height (0.4): the end fades by 25%.
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, x, at(3.0, 0.5).1);
        let Edit::Tilt { left, right, from_top } = edit else { panic!("not a tilt") };
        assert!(left == 1.0 && (right - 0.75).abs() < 1e-6 && !from_top);
        assert_eq!(words, "fade out 25%");
        // Up from where it was: nothing fades.
        let (edit, _) = drag(h, s.length(), f, GRID, NO_KEYS, x, at(3.0, 0.9).1);
        assert_eq!(edit, Edit::Tilt { left: 1.0, right: 1.0, from_top: false });
        // A bottom left corner pushed up fades in from the top.
        let (x, y) = at(1.0, 0.2);
        let h = hold(&s, lit(), Part::Corner { left: true, top: false }, x, y);
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, x, at(1.0, 0.4).1);
        let Edit::Tilt { left, right, from_top } = edit else { panic!("not a tilt") };
        assert!((left - 0.5).abs() < 1e-6 && right == 1.0 && from_top);
        assert_eq!(words, "fade in 50%");
    }

    #[test]
    fn the_bar_moves_copies_and_smooths() {
        let (s, f) = (shape(), frame());
        let (x, y) = (at(2.0, 0.0).0, 6.0);
        let h = hold(&s, lit(), Part::Bar, x, y);
        let (edit, words) = drag(h, s.length(), f, GRID, NO_KEYS, x + 100.0, y);
        assert_eq!((edit, words.as_str()), (Edit::Move { to: 3.0, copy: false }, "1.4"));
        let (edit, words) = drag(h, s.length(), f, GRID, CTRL, x + 100.0, y);
        assert_eq!((edit, words.as_str()), (Edit::Move { to: 3.0, copy: true }, "copy to 1.4"));
        // Past the end it stops where it still fits: the read-out says where that is.
        let (_, words) = drag(h, s.length(), f, GRID, NO_KEYS, x + 1000.0, y);
        assert_eq!(words, "2.3");
        // Alt: up smooths, down sharpens.
        let (edit, words) = drag(h, s.length(), f, GRID, ALT, x, y - 40.0);
        assert_eq!((edit, words.as_str()), (Edit::Smooth { amount: 0.5, short: 1.0 }, "smoother 50%"));
        let (edit, words) = drag(h, s.length(), f, GRID, ALT, x, y + 200.0);
        assert_eq!((edit, words.as_str()), (Edit::Smooth { amount: -1.0, short: 1.0 }, "sharper 100%"));
    }

    #[test]
    fn empty_space_is_dragged_to_a_highlight() {
        let f = frame();
        let span = swept(f, GRID, NO_KEYS, 8.0, at(2.1, 0.0).0, at(4.9, 0.0).0);
        assert_eq!(span, Span::new(2.0, 5.0));
        assert_eq!(swept_words(span), "1.3 · 3 beats");
        // Right to left is the same stretch; past the end it stops at the end.
        assert_eq!(swept(f, GRID, NO_KEYS, 8.0, at(4.9, 0.0).0, at(2.1, 0.0).0), Span::new(2.0, 5.0));
        assert_eq!(swept(f, GRID, SHIFT, 8.0, at(6.5, 0.0).0, at(20.0, 0.0).0), Span::new(6.5, 8.0));
        assert_eq!(swept_words(Span::new(0.0, 8.0)), "1.1 · 2 bars");
        assert_eq!(swept_words(Span::new(4.0, 4.5)), "2.1 · 1/2 beat");
    }

    #[test]
    fn every_handle_has_its_hint() {
        for part in [Part::Bar, Part::Left, Part::Top, Part::Bottom, Part::Corner { left: true, top: true }] {
            let h = hint(Over::Part(part), false);
            assert!(!h.name.is_empty() && !h.does.is_empty() && !h.keys.is_empty());
        }
        assert!(hint(Over::Part(Part::Bar), true).keys.contains("Option"));
        assert!(hint(Over::Empty, false).does.contains("drag to highlight"));
        assert_eq!(hint(Over::Dot(0), false), curve_touch::hint(Under::Dot(0), false));
    }
}
