//! Painting on the curve editor's picture (step 7 of the plan Solomon approved on 14 Nov 2026): Ctrl-drag over
//! empty space paints steps, one a grid square; with Shift too (or the grid off) it draws a line by hand. And the
//! swing mark on the ruler: every second grid line dragged later or earlier. No screen here: a stroke gathers where
//! the mouse has been and gives the one edit it comes to, so a whole stroke is one step of undo.

use std::collections::BTreeMap;

use crate::curve_edit::{Edit, Grid, MOST_SWING};
use crate::curve_picture::Frame;
use crate::curve_touch::{Keys, percent};

/// How far the mouse may stray from a straight run before a drawn line keeps a point there, in points.
const DRAWN_CLOSE: f64 = 1.5;

/// A stroke in hand: steps on the grid (which square, and its value), or a line drawn freely. It keeps beats and
/// values, not places on the screen, so zooming or moving along mid-stroke leaves what's drawn where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    /// The grid's step in beats; none when it's a line by hand.
    step: Option<f64>,
    cells: BTreeMap<i64, f64>,
    /// The line by hand, in the order of time: (beat, value).
    line: Vec<(f64, f64)>,
    last: Option<(f64, f64)>,
}

impl Stroke {
    /// A stroke begun with these keys on this grid: steps, or a line by hand with Shift held or the grid off.
    pub fn begin(grid: Grid, keys: Keys) -> Stroke {
        let step = if keys.shift { None } else { grid.beats() };
        Stroke { step, cells: BTreeMap::new(), line: Vec::new(), last: None }
    }

    /// Whether it draws by hand.
    pub fn by_hand(&self) -> bool {
        self.step.is_none()
    }

    /// The mouse is at `x`, `y` on the picture of a curve `length` beats long: past its ends counts as on them.
    pub fn add(&mut self, frame: Frame, length: f64, x: f64, y: f64) {
        if !x.is_finite() || !y.is_finite() || !length.is_finite() || length <= 0.0 {
            return;
        }
        let beat = frame.beat_at(x).clamp(0.0, length);
        let value = frame.value_at(y).clamp(0.0, 1.0);
        match self.step {
            Some(step) => {
                // The last square is the one that ends at the curve's end, even with the mouse right on it.
                let most = ((length / step) - 1e-9).ceil().max(1.0) as i64 - 1;
                let cell = |beat: f64| ((beat / step + 1e-9).floor() as i64).clamp(0, most);
                let now = cell(beat);
                // The squares the mouse skipped on its way here get the values on the way.
                match self.last {
                    Some((last_beat, last_value)) if cell(last_beat) != now => {
                        let from = cell(last_beat);
                        let n = (now - from).abs();
                        for i in 1..=n {
                            let k = from + (now - from).signum() * i;
                            self.cells.insert(k, last_value + (value - last_value) * i as f64 / n as f64);
                        }
                    }
                    _ => {
                        self.cells.insert(now, value);
                    }
                }
            }
            None => {
                // Going back over time it has drawn, the new way takes the old one's place.
                if let Some((last_beat, _)) = self.last {
                    let (low, high) = (last_beat.min(beat), last_beat.max(beat));
                    self.line.retain(|p| p.0 < low || p.0 > high || p.0 == last_beat);
                }
                self.line.retain(|p| p.0 != beat);
                let at = self.line.partition_point(|p| p.0 < beat);
                self.line.insert(at, (beat, value));
            }
        }
        self.last = Some((beat, value));
    }

    /// The edit the stroke comes to so far; none before there's anything of it. The picture as it is now says how
    /// close a drawn line's points may be to a straight run and still be left out.
    pub fn edit(&self, frame: Frame) -> Option<Edit> {
        match self.step {
            Some(step) => {
                let cells: Vec<(f64, f64, f64)> =
                    self.cells.iter().map(|(k, v)| (*k as f64 * step, (*k + 1) as f64 * step, *v)).collect();
                (!cells.is_empty()).then_some(Edit::Paint { cells })
            }
            None => {
                let on_screen: Vec<(f64, f64)> = self.line.iter().map(|&(b, v)| (frame.x(b), frame.y(v))).collect();
                let keep = thinned(&on_screen, DRAWN_CLOSE);
                let line: Vec<(f64, f64)> = self.line.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect();
                (line.len() >= 2).then_some(Edit::Draw { line })
            }
        }
    }

    /// The read-out beside the mouse while it paints.
    pub fn words(&self) -> String {
        let Some((_, value)) = self.last else { return String::new() };
        let value = percent(value);
        match self.step {
            Some(_) if self.cells.len() == 1 => format!("1 step · {value}"),
            Some(_) => format!("{} steps · {value}", self.cells.len()),
            None => format!("drawing · {value}"),
        }
    }
}

/// Which points of a line say its shape, in order: those further than `close` from the straight run between their
/// neighbours (Ramer, Douglas and Peucker's way), the ends always.
fn thinned(line: &[(f64, f64)], close: f64) -> Vec<bool> {
    let mut keep = vec![false; line.len()];
    if line.len() < 3 {
        keep.fill(true);
        return keep;
    }
    keep[0] = true;
    keep[line.len() - 1] = true;
    let mut runs = vec![(0usize, line.len() - 1)];
    while let Some((a, b)) = runs.pop() {
        let ((ax, ay), (bx, by)) = (line[a], line[b]);
        let run = (bx - ax).hypot(by - ay).max(1e-9);
        let far = (a + 1..b)
            .map(|i| (i, ((line[i].0 - ax) * (by - ay) - (line[i].1 - ay) * (bx - ax)).abs() / run))
            .max_by(|l, r| l.1.total_cmp(&r.1));
        if let Some((i, d)) = far
            && d > close
        {
            keep[i] = true;
            runs.push((a, i));
            runs.push((i, b));
        }
    }
    keep
}

/// What the hint line says with the mouse over empty space and the paint key held; `in_hand` while a stroke is
/// being painted (its kind was settled when it began).
pub fn hint(grid: Grid, keys: Keys, in_hand: bool) -> (&'static str, &'static str) {
    let by_hand = keys.shift || grid.beats().is_none();
    let does = if by_hand { "drag draws a line by hand" } else { "drag paints steps, one a grid square" };
    let more = match (in_hand, by_hand, grid.beats().is_some()) {
        (true, _, _) => "Esc puts it back",
        (false, false, _) => "with Shift: by hand",
        (false, true, true) => "let go of Shift: steps on the grid",
        (false, true, false) => "turn the grid on to paint steps",
    };
    (does, more)
}

/// A swing mark dragged from the beat `from` to `x` on the picture: how far every second line of a grid `step`
/// beats apart moves, of a step (later above 0), never past [`MOST_SWING`]. With Shift it moves freely; else it
/// catches on no swing, on triplet swing (a third) and on a half when it's close.
pub fn swing(frame: Frame, step: f64, keys: Keys, from: f64, x: f64) -> f64 {
    if !step.is_finite() || step <= 0.0 || !x.is_finite() || !from.is_finite() {
        return 0.0;
    }
    let amount = ((frame.beat_at(x) - from) / step).clamp(-MOST_SWING, MOST_SWING);
    if keys.shift {
        return amount;
    }
    const CATCH: f64 = 0.04;
    [0.0, 1.0 / 3.0, -1.0 / 3.0, 0.5, -0.5].into_iter().find(|c| (amount - c).abs() < CATCH).unwrap_or(amount)
}

/// Swing in words, as drum machines count it: 50% is straight, 67% triplet swing.
pub fn swing_words(amount: f64) -> String {
    let percent = (50.0 * (1.0 + amount)).round();
    if amount.abs() < 1e-9 {
        "no swing".to_owned()
    } else if (amount.abs() - 1.0 / 3.0).abs() < 1e-9 {
        format!("swing {percent:.0}% · triplet{}", if amount < 0.0 { ", early" } else { "" })
    } else {
        format!("swing {percent:.0}%{}", if amount < 0.0 { " · early" } else { "" })
    }
}

/// Where the swing marks stand, in beats: on the curve's odd grid lines (a grid `step` beats apart, counted from
/// the curve's start) inside `start` to `end` (the highlight, or the whole curve), those in sight only. Where they'd
/// be closer than `apart` points, only every so many are given: they move as one anyway.
pub fn swing_marks(step: f64, start: f64, end: f64, frame: Frame, apart: f64) -> Vec<f64> {
    let width = frame.beat_width();
    if !step.is_finite()
        || step <= 0.0
        || !width.is_finite()
        || width <= 0.0
        || end.partial_cmp(&start) != Some(std::cmp::Ordering::Greater)
    {
        return Vec::new();
    }
    let pair = 2.0 * step;
    let every = (apart / (pair * width)).ceil().max(1.0);
    let view = frame.view;
    let (low, high) = (start.max(view.start), end.min(view.start + view.span));
    // The mark numbered k stands at (2k + 1) steps; those shown are every so many, counted from the curve's start.
    let first = (((low - step) / pair / every) - 1e-9).ceil().max(0.0) * every;
    let mut out = Vec::new();
    let mut k = first;
    while out.len() < 4096 {
        let beat = (2.0 * k + 1.0) * step;
        if beat >= high - 1e-9 {
            break;
        }
        if beat > low + 1e-9 {
            out.push(beat);
        }
        k += every;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve_edit::{Shape, Step, apply};
    use crate::curve_picture::View;
    use mo_formats::bitwig::curve::CurvePoint;

    fn frame() -> Frame {
        Frame { width: 428.0, height: 236.0, view: View::whole(4.0) }
    }

    fn grid(step: Step) -> Grid {
        Grid { step: Some(step), triplet: false, dotted: false }
    }

    fn ctrl() -> Keys {
        Keys { ctrl: true, ..Keys::default() }
    }

    #[test]
    fn a_stroke_paints_a_step_in_each_square_it_crosses() {
        let f = frame();
        let mut s = Stroke::begin(grid(Step::Quarter), ctrl());
        assert!(s.edit(f).is_none());
        s.add(f, 4.0, f.x(0.5), f.y(0.2));
        // Straight to the fourth beat: the two squares between are filled on the way.
        s.add(f, 4.0, f.x(3.5), f.y(0.8));
        let Some(Edit::Paint { cells }) = s.edit(f) else { panic!("steps") };
        assert_eq!(cells.len(), 4);
        for (i, (start, end, value)) in cells.iter().enumerate() {
            assert!((start - i as f64).abs() < 1e-9 && (end - start - 1.0).abs() < 1e-9);
            assert!((value - (0.2 + 0.2 * i as f64)).abs() < 0.01, "{value}");
        }
        assert_eq!(s.words(), "4 steps · 80%");
        // Past the curve's end counts as its last square: no steps are counted that won't be painted.
        s.add(f, 4.0, f.x(4.0) + 300.0, f.y(0.8));
        assert_eq!(s.words(), "4 steps · 80%");
        // Back over a square: its new value.
        s.add(f, 4.0, f.x(2.5), f.y(0.1));
        let Some(Edit::Paint { cells }) = s.edit(f) else { panic!("steps") };
        assert!((cells[2].2 - 0.1).abs() < 0.01);
        // The whole stroke is one edit the rules take.
        let flat = [CurvePoint { x: 0.0, y: 0.5, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.5, bend: 0.0 }];
        let shape = Shape::from_curve(&flat, 4.0);
        let (after, _) = apply(&shape, None, &Edit::Paint { cells }).expect("painted");
        assert!((after.value_at(0.5) - 0.2).abs() < 0.01 && (after.value_at(2.5) - 0.1).abs() < 0.01);
    }

    #[test]
    fn a_line_by_hand_keeps_only_the_points_that_say_its_shape() {
        let f = frame();
        let mut s = Stroke::begin(grid(Step::Quarter), Keys { ctrl: true, shift: true, alt: false });
        assert!(s.by_hand());
        // Up in a straight run, then down: three points say it all.
        for i in 0..=20 {
            s.add(f, 4.0, f.x(1.0) + f64::from(i) * 4.0, f.y(0.2) - f64::from(i) * 3.0);
        }
        for i in 1..=20 {
            s.add(f, 4.0, f.x(1.0) + 80.0 + f64::from(i) * 4.0, f.y(0.2) - 60.0 + f64::from(i) * 3.0);
        }
        let Some(Edit::Draw { line }) = s.edit(f) else { panic!("a line") };
        assert_eq!(line.len(), 3, "{line:?}");
        assert!(line.windows(2).all(|w| w[0].0 < w[1].0));
        // Back over what it drew: the new way takes its place, and time still runs one way.
        s.add(f, 4.0, f.x(1.0) + 40.0, f.y(0.9));
        // The view moved along mid-stroke: what was drawn stays at its own beats.
        let moved = Frame { view: View { start: 1.0, span: 2.0 }, ..f };
        let Some(Edit::Draw { line: after }) = s.edit(moved) else { panic!("a line") };
        assert!((after[0].0 - 1.0).abs() < 1e-6, "{after:?}");
        let Some(Edit::Draw { line }) = s.edit(f) else { panic!("a line") };
        assert!(line.windows(2).all(|w| w[0].0 < w[1].0), "{line:?}");
        assert!(line.iter().any(|p| (p.1 - 0.9).abs() < 0.01));
        // With the grid off, Ctrl alone draws by hand.
        assert!(Stroke::begin(Grid { step: None, triplet: false, dotted: false }, ctrl()).by_hand());
    }

    #[test]
    fn the_swing_mark_catches_on_triplet_swing_and_stops_at_three_quarters() {
        let f = frame();
        let x = |beat: f64| f.x(beat);
        let none = Keys::default();
        assert!((swing(f, 1.0, none, 1.0, x(1.34)) - 1.0 / 3.0).abs() < 1e-9);
        assert!((swing(f, 1.0, Keys { shift: true, ..none }, 1.0, x(1.34)) - 0.34).abs() < 1e-6);
        assert!((swing(f, 1.0, none, 1.0, x(1.2)) - 0.2).abs() < 1e-6);
        assert!((swing(f, 1.0, none, 1.0, x(3.0)) - MOST_SWING).abs() < 1e-9);
        assert!((swing(f, 1.0, none, 1.0, x(0.5)) + 0.5).abs() < 1e-9);
        assert_eq!(swing(f, 0.0, none, 1.0, x(2.0)), 0.0);
        assert_eq!(swing_words(0.0), "no swing");
        assert_eq!(swing_words(1.0 / 3.0), "swing 67% · triplet");
        assert_eq!(swing_words(0.2), "swing 60%");
        assert_eq!(swing_words(-0.2), "swing 40% · early");
        assert_eq!(swing_marks(1.0, 0.0, 4.0, f, 28.0), [1.0, 3.0]);
        // A highlight that starts off the pairs: the marks are still the curve's odd lines.
        assert_eq!(swing_marks(1.0, 0.5, 3.5, f, 28.0), [1.0, 3.0]);
        // Too close to tell apart: only every so many, never none.
        let thin = swing_marks(1.0 / 64.0, 0.0, 4.0, f, 28.0);
        assert!(!thin.is_empty() && thin.len() < 20, "{}", thin.len());
        assert!(thin.windows(2).all(|w| (w[1] - w[0]) * f.beat_width() >= 28.0 - 1e-6));
        // Far along a long curve, zoomed in: the marks in sight are found without counting from its start.
        let far = Frame { view: View { start: 200.0, span: 2.0 }, ..f };
        let marks = swing_marks(1.0 / 8.0, 0.0, 256.0, far, 28.0);
        assert!(!marks.is_empty() && marks.iter().all(|m| *m > 200.0 && *m < 202.0), "{marks:?}");
    }

    #[test]
    fn the_hint_says_what_the_paint_key_does() {
        let g = grid(Step::Sixteenth);
        let both = Keys { ctrl: true, shift: true, alt: false };
        let off = Grid { step: None, triplet: false, dotted: false };
        assert_eq!(hint(g, ctrl(), false), ("drag paints steps, one a grid square", "with Shift: by hand"));
        assert_eq!(hint(g, both, false), ("drag draws a line by hand", "let go of Shift: steps on the grid"));
        assert_eq!(hint(off, ctrl(), false).1, "turn the grid on to paint steps");
        // A stroke in hand keeps its kind: nothing is said about keys that no longer change it.
        assert_eq!(hint(g, both, true).1, "Esc puts it back");
    }
}
