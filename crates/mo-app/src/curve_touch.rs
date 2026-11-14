//! What the mouse does on the curve editor's picture, worked out with no screen (the screen design of 14 Nov 2026,
//! "Points and lines" and "Hint line, key lamps and read-out"): which dots show, what's under the mouse, the edit a
//! drag or a double-click makes ([`crate::curve_edit::Edit`]), the hint line's words and the read-out by the mouse.
//! Three keys change a gesture, the same everywhere: Shift is free (off the grid), Alt bends, Ctrl makes.

use crate::curve_edit::{Edit, Grid, Shape};
use crate::curve_picture::{BAR, Frame};

/// Dots closer than this to the last one shown aren't shown (nothing smaller than 24 points can be grabbed), in
/// points; the hidden ones show as the mouse comes within [`NEAR_DOT`].
pub const DOT_APART: f64 = 24.0;
/// How near the mouse must be to a dot to have it (its grab area is 24 across), and to a line to light it.
pub const NEAR_DOT: f64 = 12.0;
pub const NEAR_LINE: f64 = 6.0;
/// How far an Alt-drag goes to bend a line all the way, as a part of the picture's height.
const BEND_REACH: f64 = 0.5;

/// The keys held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// What's under the mouse: a point (its place in [`Shape::points`]), the line that starts at a point, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Under {
    Dot(usize),
    Line(usize),
    Empty,
}

/// A dot on show: its point and where, in points from the picture's left and top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub point: usize,
    pub x: f64,
    pub y: f64,
}

/// Where each point in sight is on the screen.
fn places(shape: &Shape, frame: Frame) -> impl Iterator<Item = Dot> + '_ {
    let view = frame.view;
    shape
        .points()
        .iter()
        .enumerate()
        .filter(move |(_, p)| p.beat >= view.start - 1e-9 && p.beat <= view.start + view.span + 1e-9)
        .map(move |(point, p)| Dot { point, x: frame.x(p.beat), y: frame.y(p.value) })
}

fn apart(a: &Dot, x: f64, y: f64) -> f64 {
    (a.x - x).hypot(a.y - y)
}

/// The dots on show: from the left, each at least [`DOT_APART`] from the last one shown, so a dense gate shows the
/// corners of its steps and not a bead on every one; and the one nearest the mouse within [`NEAR_DOT`], whatever its
/// neighbours, so every point can be reached.
pub fn dots(shape: &Shape, frame: Frame, mouse: Option<(f64, f64)>) -> Vec<Dot> {
    let near = mouse.and_then(|(x, y)| nearest_dot(shape, frame, x, y));
    dots_with(shape, frame, near)
}

/// [`dots`] with the point that shows whatever its neighbours given (`near`): the one under the mouse, or in hand.
pub fn dots_with(shape: &Shape, frame: Frame, near: Option<usize>) -> Vec<Dot> {
    if !frame.fits() {
        return Vec::new();
    }
    let mut shown: Vec<Dot> = Vec::new();
    for d in places(shape, frame) {
        let room = shown.last().is_none_or(|last| apart(last, d.x, d.y) >= DOT_APART);
        if room || near == Some(d.point) {
            shown.push(d);
        }
    }
    shown
}

/// The point in sight nearest `x`, `y`, and how far it is.
fn nearest(shape: &Shape, frame: Frame, x: f64, y: f64) -> Option<(f64, usize)> {
    places(shape, frame).map(|d| (apart(&d, x, y), d.point)).min_by(|a, b| a.0.total_cmp(&b.0))
}

/// The point whose dot the mouse has: the nearest within [`NEAR_DOT`].
fn nearest_dot(shape: &Shape, frame: Frame, x: f64, y: f64) -> Option<usize> {
    nearest(shape, frame, x, y).filter(|(far, _)| *far <= NEAR_DOT).map(|(_, point)| point)
}

/// The point a dragged dot is now, in the curve as the drag's `edit` made it from `before`: the one where the edit
/// put it (the grid, its neighbours and the picture's edges keep a point from following the mouse all the way, and a
/// point that joined a neighbour has another place in the list).
pub fn held_point(shape: &Shape, before: &Shape, edit: &Edit) -> Option<usize> {
    let Edit::MovePoint { point, beat, value } = *edit else { return None };
    let points = before.points();
    let low = point.checked_sub(1).and_then(|i| points.get(i)).map_or(0.0, |p| p.beat);
    let high = points.get(point + 1).map_or(before.length(), |p| p.beat);
    let (beat, value) = (beat.clamp(low, high.max(low)), value.clamp(0.0, 1.0));
    shape
        .points()
        .iter()
        .enumerate()
        .filter(|(_, p)| (p.beat - beat).abs() < 1e-6 && (p.value - value).abs() < 1e-6)
        .min_by_key(|(i, _)| i.abs_diff(point))
        .map(|(i, _)| i)
}

/// The line a held line is now: the one that starts where it started when the drag began (`before`), as a point
/// that joined a neighbour moves the ones after it up the list.
pub fn held_line(shape: &Shape, before: &Shape, grab: Grab) -> Option<usize> {
    let Grab::Line { line, .. } = grab else { return None };
    let from = before.points().get(line)?.beat;
    let now = shape.points().iter().rposition(|p| p.beat <= from + 1e-9)?;
    shape.points().get(now + 1).map(|_| now)
}

/// The line under the mouse: the one over its beat, when the mouse is within [`NEAR_LINE`] of it up or down. A jump
/// (two points at one beat) is no line to hold.
fn line_at(shape: &Shape, frame: Frame, x: f64, y: f64) -> Option<usize> {
    let beat = frame.beat_at(x);
    let points = shape.points();
    let line = points.iter().rposition(|p| p.beat <= beat)?;
    let next = points.get(line + 1)?;
    let from = points.get(line)?;
    if next.beat - from.beat < 1e-9 || beat > next.beat {
        return None;
    }
    ((frame.y(shape.line_value(line, beat)) - y).abs() <= NEAR_LINE).then_some(line)
}

/// What's under the mouse at `x`, `y` (points from the picture's left and top): a dot wins over a line.
pub fn under(shape: &Shape, frame: Frame, x: f64, y: f64) -> Under {
    if !frame.fits() || !x.is_finite() || !y.is_finite() {
        return Under::Empty;
    }
    if let Some(point) = nearest_dot(shape, frame, x, y) {
        return Under::Dot(point);
    }
    line_at(shape, frame, x, y).map_or(Under::Empty, Under::Line)
}

/// What a press took hold of, with what a drag needs of how things were then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grab {
    /// A point, and how far the press was from its middle (so it doesn't jump to the mouse).
    Dot { point: usize, off_x: f64, off_y: f64 },
    /// A line: where the press was, and the line's bend then.
    Line { line: usize, y: f64, bend: f64 },
}

/// What a press at `x`, `y` takes hold of; nothing on empty space (that drag is the highlight's).
pub fn grab(shape: &Shape, frame: Frame, x: f64, y: f64) -> Option<Grab> {
    match under(shape, frame, x, y) {
        Under::Dot(point) => {
            let p = shape.points().get(point)?;
            Some(Grab::Dot { point, off_x: x - frame.x(p.beat), off_y: y - frame.y(p.value) })
        }
        Under::Line(line) => Some(Grab::Line { line, y, bend: shape.points().get(line)?.bend }),
        Under::Empty => None,
    }
}

/// The edit a drag makes with the mouse now at `x`, `y`, and the read-out's words for it. The edit is made on the
/// curve as it was when the drag began (`before`, as `Editor::drag` does it). A point goes to the grid unless Shift
/// is held; a line moves up or down, or with Alt bends: up early, down late. The read-out says where the point or
/// line gets to (a point stops at its neighbours, a line at the top and bottom), not where the mouse is.
pub fn drag(grab: Grab, before: &Shape, frame: Frame, grid: Grid, keys: Keys, x: f64, y: f64) -> (Edit, String) {
    let points = before.points();
    match grab {
        Grab::Dot { point, off_x, off_y } => {
            let beat = grid.snap(frame.beat_at(x - off_x), keys.shift).max(0.0);
            let value = frame.value_at(y - off_y);
            // Where it lands, as `Edit::MovePoint` keeps it between its neighbours.
            let low = point.checked_sub(1).and_then(|i| points.get(i)).map_or(0.0, |p| p.beat);
            let high = points.get(point + 1).map_or(before.length(), |p| p.beat);
            let lands = beat.clamp(low, high.max(low));
            (Edit::MovePoint { point, beat, value }, format!("{} · {}", place_words(lands), percent(value)))
        }
        Grab::Line { line, y: from, bend } if keys.alt => {
            // Up is early: the line gets most of the way sooner, which is a bend below 0.
            let up = (from - y) / (frame.height - 2.0 * crate::curve_picture::TOP).max(1.0);
            let bend = (bend - up / BEND_REACH).clamp(-1.0, 1.0);
            (Edit::Bend { line, bend }, bend_words(bend))
        }
        Grab::Line { line, y: from, .. } => {
            let by = frame.value_at(y) - frame.value_at(from);
            // How far it goes, as `Edit::MoveLine` stops it at the top and bottom.
            let moved = match (points.get(line), points.get(line + 1)) {
                (Some(a), Some(b)) => by.clamp(-a.value.min(b.value), 1.0 - a.value.max(b.value)),
                _ => by,
            };
            let words = format!("{}{}", if moved >= 0.0 { "+" } else { "−" }, percent(moved.abs()));
            (Edit::MoveLine { line, by }, words)
        }
    }
}

/// The edit a double-click at `x`, `y` makes: on a dot it goes; on a line with Alt the line is straightened; on a
/// line or empty space a point is added at the mouse (on the grid unless Shift is held; on a line it stays on the
/// line's own height, so the shape isn't changed until it's moved). None where there's nothing to do.
pub fn double_click(shape: &Shape, frame: Frame, grid: Grid, keys: Keys, x: f64, y: f64) -> Option<Edit> {
    match under(shape, frame, x, y) {
        Under::Dot(point) => Some(Edit::RemovePoint(point)),
        Under::Line(line) if keys.alt => Some(Edit::Bend { line, bend: 0.0 }),
        hit => {
            let beat = grid.snap(frame.beat_at(x), keys.shift);
            if !(0.0..=shape.length()).contains(&beat) {
                return None;
            }
            let value = match hit {
                Under::Line(_) => shape.value_at(beat),
                _ => frame.value_at(y),
            };
            Some(Edit::AddPoint { beat, value })
        }
    }
}

/// The hint line: the name of what's under the mouse, what it does, and what the keys would change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    pub name: &'static str,
    pub does: &'static str,
    pub keys: &'static str,
}

/// The hint line's words for what's under the mouse. `mac`: the keys are named as a Mac's are.
pub fn hint(under: Under, mac: bool) -> Hint {
    let alt = |pc: &'static str, on_mac: &'static str| if mac { on_mac } else { pc };
    match under {
        Under::Dot(_) => {
            Hint { name: "Point", does: "drag to move · double-click removes it", keys: "Shift: off the grid" }
        }
        Under::Line(_) => Hint {
            name: "Line",
            does: "drag moves it up or down · double-click adds a point",
            keys: alt(
                "Alt: bend it, up early, down late · Alt-double-click: straight",
                "Option: bend it, up early, down late · Option-double-click: straight",
            ),
        },
        Under::Empty => Hint { name: "Empty", does: "double-click adds a point", keys: "Shift: off the grid" },
    }
}

/// A beat as its place: bar, beat and sixteenth, each from 1 ("1.3.2"), the sixteenth only when it isn't the first.
/// Between sixteenths (off the grid) the beat is said to two decimals instead: "1 · beat 3.31".
pub fn place_words(beat: f64) -> String {
    let beat = if beat.is_finite() { beat.max(0.0) } else { 0.0 };
    // Counted in whole sixteenths first, so a beat a hair under a whole one is that one and not a "fifth sixteenth".
    let sixteenths = (beat * 4.0).round();
    if (beat * 4.0 - sixteenths).abs() > 1e-6 {
        let bar = (beat / BAR).floor();
        return format!("{} · beat {:.2}", bar as u64 + 1, beat - bar * BAR + 1.0);
    }
    let sixteenths = sixteenths as u64;
    let per_bar = (BAR * 4.0) as u64;
    let (bar, in_bar) = (sixteenths / per_bar + 1, sixteenths % per_bar);
    match in_bar % 4 {
        0 => format!("{bar}.{}", in_bar / 4 + 1),
        n => format!("{bar}.{}.{}", in_bar / 4 + 1, n + 1),
    }
}

/// A value (0 to 1) as a whole percent.
pub fn percent(value: f64) -> String {
    format!("{}%", (value * 100.0).round() as i64)
}

/// A bend in words: "bend 60% early", "bend 20% late", "straight".
fn bend_words(bend: f64) -> String {
    let part = (bend.abs() * 100.0).round() as i64;
    if part == 0 {
        "straight".to_owned()
    } else {
        format!("bend {part}% {}", if bend < 0.0 { "early" } else { "late" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve_edit::Step;
    use crate::curve_picture::{SIDE, TOP, View};
    use mo_formats::bitwig::curve::CurvePoint;

    /// A bar: up to the top at beat 2, a jump down to a quarter there, then level.
    fn shape() -> Shape {
        let p = |x, y, bend| CurvePoint { x, y, bend };
        Shape::from_curve(&[p(0.0, 0.0, 0.0), p(0.5, 1.0, 0.0), p(0.5, 0.25, 0.0), p(1.0, 0.25, 0.0)], 4.0)
    }

    /// 100 points a beat and 200 points of height, so places are easy to say.
    fn frame() -> Frame {
        Frame { width: 400.0 + 2.0 * SIDE, height: 200.0 + 2.0 * TOP, view: View::whole(4.0) }
    }

    fn at(beat: f64, value: f64) -> (f64, f64) {
        (frame().x(beat), frame().y(value))
    }

    const NO_KEYS: Keys = Keys { shift: false, alt: false, ctrl: false };
    const SHIFT: Keys = Keys { shift: true, alt: false, ctrl: false };
    const ALT: Keys = Keys { shift: false, alt: true, ctrl: false };

    #[test]
    fn a_dot_wins_over_its_line_and_a_jump_is_no_line() {
        let (s, f) = (shape(), frame());
        let (x, y) = at(0.0, 0.0);
        assert_eq!(under(&s, f, x + 8.0, y - 8.0), Under::Dot(0));
        // The jump's two dots sit at one beat: the nearer one is had.
        let (x, y) = at(2.0, 1.0);
        assert_eq!(under(&s, f, x, y + 4.0), Under::Dot(1));
        let (x, y) = at(2.0, 0.25);
        assert_eq!(under(&s, f, x, y - 4.0), Under::Dot(2));
        // Half way down the jump: neither dot is near, and a jump can't be held.
        let (x, y) = at(2.0, 0.62);
        assert_eq!(under(&s, f, x, y), Under::Empty);
        // On the rise, and just off it; on the level line; far from both.
        let (x, y) = at(1.0, 0.5);
        assert_eq!(under(&s, f, x, y + 5.0), Under::Line(0));
        assert_eq!(under(&s, f, x, y + 9.0), Under::Empty);
        let (x, y) = at(3.0, 0.25);
        assert_eq!(under(&s, f, x, y), Under::Line(2));
        let (x, y) = at(3.0, 0.8);
        assert_eq!(under(&s, f, x, y), Under::Empty);
        assert_eq!(under(&s, f, f64::NAN, 3.0), Under::Empty);
    }

    #[test]
    fn close_dots_hide_until_the_mouse_comes() {
        // Sixteen steps in a bar at 25 points a step: every corner is two dots, 0 or 150 points apart up and down.
        let mut points = Vec::new();
        for i in 0..16 {
            let x = f64::from(i) / 16.0;
            points.push(CurvePoint { x, y: 1.0, bend: 0.0 });
            points.push(CurvePoint { x: x + 1.0 / 32.0, y: 1.0, bend: 0.0 });
        }
        let gate = Shape::from_curve(&points, 4.0);
        let f = frame();
        let shown = dots(&gate, f, None);
        assert!(shown.len() < gate.points().len(), "{} of {}", shown.len(), gate.points().len());
        for pair in shown.windows(2) {
            assert!(apart(&pair[0], pair[1].x, pair[1].y) >= DOT_APART - 1e-9);
        }
        // A hidden one shows when the mouse is on it, and can be had.
        let hidden = (0..gate.points().len()).find(|i| shown.iter().all(|d| d.point != *i)).unwrap();
        let p = gate.points()[hidden];
        let (x, y) = (f.x(p.beat), f.y(p.value));
        assert!(dots(&gate, f, Some((x + 2.0, y))).iter().any(|d| d.point == hidden));
        assert_eq!(under(&gate, f, x + 2.0, y), Under::Dot(hidden));
        // Zoomed in, only the points in sight have dots.
        let close = Frame { view: View { start: 1.0, span: 1.0 }, ..f };
        assert!(dots(&gate, close, None).iter().all(|d| d.x >= SIDE - 1e-6 && d.x <= close.width - SIDE + 1e-6));
        assert!(dots(&gate, Frame { width: 0.0, ..f }, None).is_empty());
    }

    #[test]
    fn a_dragged_point_lands_on_the_grid_unless_shift_is_held() {
        let (s, f) = (shape(), frame());
        let grid = Grid { step: Some(Step::Sixteenth), triplet: false, dotted: false };
        let (x, y) = at(2.0, 1.0);
        // Held 3 points off its middle: it doesn't jump to the mouse.
        let g = grab(&s, f, x + 3.0, y + 3.0).unwrap();
        assert!(matches!(g, Grab::Dot { point: 1, .. }));
        let (to_x, to_y) = at(1.31, 0.6);
        let (edit, words) = drag(g, &s, f, grid, NO_KEYS, to_x + 3.0, to_y + 3.0);
        assert_eq!(words, "1.2.2 · 60%");
        let Edit::MovePoint { point: 1, beat, value } = edit else { panic!("{edit:?}") };
        assert!((beat - 1.25).abs() < 1e-9 && (value - 0.6).abs() < 1e-9);
        let (edit, words) = drag(g, &s, f, grid, SHIFT, to_x + 3.0, to_y + 3.0);
        let Edit::MovePoint { beat, .. } = edit else { panic!("{edit:?}") };
        assert!((beat - 1.31).abs() < 1e-9);
        assert_eq!(words, "1 · beat 2.31 · 60%");
        // Past its neighbour it stops there, and the read-out says where it stopped; the held point is still found
        // though the mouse is far from it.
        let (far_x, far_y) = at(3.5, 0.6);
        let (edit, words) = drag(g, &s, f, grid, NO_KEYS, far_x + 3.0, far_y + 3.0);
        assert_eq!(words, "1.3 · 60%");
        let moved = crate::curve_edit::apply(&s, None, &edit).unwrap().0;
        let held = held_point(&moved, &s, &edit).unwrap();
        assert!((moved.points()[held].beat - 2.0).abs() < 1e-9 && (moved.points()[held].value - 0.6).abs() < 1e-9);
        assert!(dots_with(&moved, f, Some(held)).iter().any(|d| d.point == held));
        // Nothing is held on empty space.
        let (x, y) = at(3.0, 0.9);
        assert_eq!(grab(&s, f, x, y), None);
    }

    #[test]
    fn a_dragged_line_moves_or_with_alt_bends() {
        let (s, f) = (shape(), frame());
        let grid = Grid::default();
        let (x, y) = at(3.0, 0.25);
        let g = grab(&s, f, x, y).unwrap();
        assert_eq!(g, Grab::Line { line: 2, y, bend: 0.0 });
        // 40 points up of 200 is a fifth.
        let (edit, words) = drag(g, &s, f, grid, NO_KEYS, x + 30.0, y - 40.0);
        let Edit::MoveLine { line: 2, by } = edit else { panic!("{edit:?}") };
        assert!((by - 0.2).abs() < 1e-9);
        assert_eq!(words, "+20%");
        assert_eq!(drag(g, &s, f, grid, NO_KEYS, x, y + 20.0).1, "−10%");
        // It stops at the bottom (it was at a quarter), and the read-out says how far it went.
        assert_eq!(drag(g, &s, f, grid, NO_KEYS, x, y + 150.0).1, "−25%");
        assert_eq!(held_line(&s, &s, g), Some(2));
        assert_eq!(held_point(&s, &s, &edit), None);
        // Alt: up bends early (below 0), down late; a quarter of the height is half way, and it stops at all the way.
        let (edit, words) = drag(g, &s, f, grid, ALT, x, y - 50.0);
        assert_eq!(edit, Edit::Bend { line: 2, bend: -0.5 });
        assert_eq!(words, "bend 50% early");
        assert_eq!(
            drag(g, &s, f, grid, ALT, x, y + 400.0),
            (Edit::Bend { line: 2, bend: 1.0 }, "bend 100% late".into())
        );
        assert_eq!(drag(g, &s, f, grid, ALT, x, y).1, "straight");
    }

    #[test]
    fn a_double_click_adds_removes_or_straightens() {
        let (s, f) = (shape(), frame());
        let grid = Grid { step: Some(Step::Quarter), triplet: false, dotted: false };
        let (x, y) = at(2.0, 1.0);
        assert_eq!(double_click(&s, f, grid, NO_KEYS, x, y), Some(Edit::RemovePoint(1)));
        // On a line the new point stays on the line, at the grid; with Alt the line goes straight instead.
        let (x, y) = at(1.1, 0.55);
        let Some(Edit::AddPoint { beat, value }) = double_click(&s, f, grid, NO_KEYS, x, y) else { panic!() };
        assert!((beat - 1.0).abs() < 1e-9 && (value - 0.5).abs() < 1e-9);
        assert_eq!(double_click(&s, f, grid, ALT, x, y), Some(Edit::Bend { line: 0, bend: 0.0 }));
        // On empty space it's where the mouse is; off the grid with Shift; nothing past the curve's end.
        let (x, y) = at(3.3, 0.9);
        let Some(Edit::AddPoint { beat, value }) = double_click(&s, f, grid, SHIFT, x, y) else { panic!() };
        assert!((beat - 3.3).abs() < 1e-9 && (value - 0.9).abs() < 1e-9);
        assert_eq!(double_click(&s, f, Grid { step: None, ..grid }, NO_KEYS, f.x(4.0) + 9.0, y), None);
    }

    #[test]
    fn places_and_hints_are_worded() {
        assert_eq!(place_words(0.0), "1.1");
        assert_eq!(place_words(6.5), "2.3.3");
        assert_eq!(place_words(4.75), "2.1.4");
        assert_eq!(place_words(f64::NAN), "1.1");
        assert_eq!(place_words(2.999_999_999_999_999_6), "1.4");
        assert_eq!(place_words(3.999_999_999_999_999_6), "2.1");
        assert_eq!(hint(Under::Dot(3), false).name, "Point");
        assert!(hint(Under::Line(0), false).keys.starts_with("Alt: bend it"));
        assert!(hint(Under::Line(0), true).keys.starts_with("Option: bend it"));
        assert_eq!(hint(Under::Empty, false).does, "double-click adds a point");
    }
}
